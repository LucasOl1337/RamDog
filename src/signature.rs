//! Assinatura digital do executável: "isto é mesmo da Microsoft?".
//!
//! Serve contra o impostor. Um `wininit.exe` fora de `System32`, sem assinatura, é malware
//! se passando por processo crítico — e nenhum gerenciador de tarefas avisa. Aqui o nome do
//! signatário vem do **certificado**, não do `CompanyName` do recurso de versão: aquele
//! qualquer um preenche com "Microsoft Corporation" e assina com um certificado próprio.
//!
//! Custo: `WinVerifyTrust` leva dezenas de milissegundos. Nunca é chamado durante a amostra —
//! só sob demanda, para o processo selecionado, numa thread, com o resultado cacheado por
//! caminho.

use std::ffi::c_void;
use std::ptr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// Assinatura presente, íntegra e encadeando até uma raiz confiável.
    Valid,
    /// Nenhuma assinatura no arquivo (nem catálogo do Windows).
    Unsigned,
    /// Assinado, mas a verificação reprovou (adulterado, expirado, raiz desconhecida).
    Invalid(String),
    /// Não deu para checar (sem acesso ao arquivo, caminho vazio).
    Unknown(String),
}

#[derive(Clone, Debug)]
pub struct SigInfo {
    pub trust: Trust,
    /// Nome do signatário lido do certificado. Vazio quando não há assinatura.
    pub signer: String,
}

impl SigInfo {
    pub fn label(&self) -> String {
        match &self.trust {
            Trust::Valid if self.signer.is_empty() => "assinatura válida".to_string(),
            Trust::Valid => format!("assinado por {}", self.signer),
            Trust::Unsigned => "sem assinatura digital".to_string(),
            Trust::Invalid(why) => format!("assinatura inválida — {why}"),
            Trust::Unknown(why) => format!("não verificado ({why})"),
        }
    }

    pub fn color(&self) -> egui::Color32 {
        match self.trust {
            Trust::Valid => egui::Color32::from_rgb(90, 220, 130),
            Trust::Unsigned => egui::Color32::from_rgb(230, 190, 80),
            Trust::Invalid(_) => egui::Color32::from_rgb(235, 90, 90),
            Trust::Unknown(_) => egui::Color32::from_rgb(140, 145, 155),
        }
    }

    pub fn tip(&self) -> &'static str {
        match self.trust {
            Trust::Valid => "O arquivo não foi alterado desde que o fabricante o assinou, e o certificado encadeia até uma raiz confiável desta máquina.",
            Trust::Unsigned => "Sem assinatura não há como provar quem fez o arquivo nem se ele foi alterado. Normal em ferramentas pequenas e em builds próprios; suspeito num executável que diz ser do Windows.",
            Trust::Invalid(_) => "A verificação reprovou: o arquivo pode ter sido adulterado, ou o certificado expirou ou não é confiável nesta máquina.",
            Trust::Unknown(_) => "Não foi possível ler o arquivo para verificar.",
        }
    }
}

#[cfg(not(windows))]
pub fn verify(_path: &str) -> SigInfo {
    SigInfo { trust: Trust::Unknown("só no Windows".into()), signer: String::new() }
}

#[cfg(windows)]
pub fn verify(path: &str) -> SigInfo {
    if path.is_empty() {
        return SigInfo { trust: Trust::Unknown("sem acesso ao caminho".into()), signer: String::new() };
    }
    let trust = check_trust(path);
    let signer = if matches!(trust, Trust::Unsigned) { String::new() } else { signer_name(path) };
    SigInfo { trust, signer }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn check_trust(path: &str) -> Trust {
    use windows::Win32::Security::WinTrust::{
        WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
        WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
        WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };

    // Códigos que o WinVerifyTrust devolve; o crate não os expõe nomeados.
    const TRUST_E_NOSIGNATURE: i32 = -2146762496; // 0x800B0100
    const TRUST_E_BAD_DIGEST: i32 = -2146869232; // 0x80096010
    const CERT_E_EXPIRED: i32 = -2146762495; // 0x800B0101
    const CERT_E_UNTRUSTEDROOT: i32 = -2146762487; // 0x800B0109
    const CERT_E_CHAINING: i32 = -2146762486; // 0x800B010A
    const TRUST_E_EXPLICIT_DISTRUST: i32 = -2146762479; // 0x800B0111

    let w = wide(path);
    unsafe {
        let mut file = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: windows::core::PCWSTR(w.as_ptr()),
            hFile: windows::Win32::Foundation::HANDLE::default(),
            pgKnownSubject: ptr::null_mut(),
        };
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
            dwStateAction: WTD_STATEACTION_VERIFY,
            // Sem ida à rede: verificação de revogação online travaria a thread por segundos.
            dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let status = WinVerifyTrust(windows::Win32::Foundation::HWND::default(), &mut action, &mut data as *mut _ as *mut c_void);

        // Fechar o estado é obrigatório: sem isso vaza contexto de confiança a cada chamada.
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        let _ = WinVerifyTrust(windows::Win32::Foundation::HWND::default(), &mut action, &mut data as *mut _ as *mut c_void);

        match status {
            0 => Trust::Valid,
            TRUST_E_NOSIGNATURE => Trust::Unsigned,
            TRUST_E_BAD_DIGEST => Trust::Invalid("arquivo alterado depois de assinado".into()),
            CERT_E_EXPIRED => Trust::Invalid("certificado expirado".into()),
            CERT_E_UNTRUSTEDROOT => Trust::Invalid("raiz não confiável".into()),
            CERT_E_CHAINING => Trust::Invalid("cadeia de certificados incompleta".into()),
            TRUST_E_EXPLICIT_DISTRUST => Trust::Invalid("certificado marcado como não confiável".into()),
            other => Trust::Invalid(format!("código 0x{:08X}", other as u32)),
        }
    }
}

/// Nome do signatário lido do certificado embutido no arquivo.
#[cfg(windows)]
fn signer_name(path: &str) -> String {
    use windows::Win32::Security::Cryptography::{
        CertCloseStore, CertFreeCertificateContext, CertGetNameStringW, CertFindCertificateInStore,
        CryptMsgClose, CryptMsgGetParam, CryptQueryObject, CERT_FIND_SUBJECT_CERT,
        CERT_NAME_SIMPLE_DISPLAY_TYPE, CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
        CERT_QUERY_FORMAT_FLAG_BINARY, CERT_QUERY_OBJECT_FILE, CMSG_SIGNER_INFO_PARAM, CMSG_SIGNER_INFO,
        CERT_QUERY_ENCODING_TYPE, HCERTSTORE, PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
    };

    let w = wide(path);
    unsafe {
        let mut store = HCERTSTORE::default();
        let mut msg: *mut c_void = ptr::null_mut();
        if CryptQueryObject(
            CERT_QUERY_OBJECT_FILE,
            w.as_ptr() as *const c_void,
            CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
            CERT_QUERY_FORMAT_FLAG_BINARY,
            0,
            None,
            None,
            None,
            Some(&mut store),
            Some(&mut msg),
            None,
        )
        .is_err()
        {
            return String::new();
        }

        let mut out = String::new();
        // Tamanho do bloco do signatário, depois o bloco em si.
        let mut need = 0u32;
        if CryptMsgGetParam(msg, CMSG_SIGNER_INFO_PARAM, 0, None, &mut need).is_ok() && need > 0 {
            let mut buf = vec![0u8; need as usize];
            if CryptMsgGetParam(msg, CMSG_SIGNER_INFO_PARAM, 0, Some(buf.as_mut_ptr() as *mut c_void), &mut need)
                .is_ok()
            {
                let si = &*(buf.as_ptr() as *const CMSG_SIGNER_INFO);
                // Localiza no store o certificado cujo emissor+série batem com o do signatário.
                let mut find = windows::Win32::Security::Cryptography::CERT_INFO {
                    Issuer: si.Issuer,
                    SerialNumber: si.SerialNumber,
                    ..Default::default()
                };
                let ctx = CertFindCertificateInStore(
                    store,
                    CERT_QUERY_ENCODING_TYPE(X509_ASN_ENCODING.0 | PKCS_7_ASN_ENCODING.0),
                    0,
                    CERT_FIND_SUBJECT_CERT,
                    Some(&mut find as *mut _ as *const c_void),
                    None,
                );
                if !ctx.is_null() {
                    let n = CertGetNameStringW(ctx, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, None);
                    if n > 1 {
                        let mut name = vec![0u16; n as usize];
                        let got = CertGetNameStringW(ctx, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut name));
                        if got > 1 {
                            out = String::from_utf16_lossy(&name[..got as usize - 1]);
                        }
                    }
                    let _ = CertFreeCertificateContext(Some(ctx));
                }
            }
        }

        let _ = CryptMsgClose(Some(msg));
        let _ = CertCloseStore(Some(store), 0);
        out
    }
}

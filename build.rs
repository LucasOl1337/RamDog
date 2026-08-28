fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/ramdog.ico");
        res.set("ProductName", "RamDog");
        res.set("FileDescription", "RamDog");

        // Elevação por padrão, só no binário que o usuário instala. Sem o manifesto o RamDog
        // só subia elevado pelo atalho que o install.ps1 marca como "executar como
        // administrador"; abrir pelo `ramdog` do PATH ou clicando no exe dava um app sem
        // temperatura de CPU e sem poder encerrar serviço nenhum, sem dizer por quê.
        //
        // Preso ao perfil release porque o recurso vale para todos os alvos do crate,
        // inclusive o binário de teste. Com o manifesto nele, `cargo test` numa sessão comum
        // morre em "a operação solicitada requer elevação" (os error 740) sem rodar um teste
        // sequer. Efeito colateral aceito: `cargo test --release` também exige sessão
        // elevada; `cargo test` (debug), que é o do dia a dia, não.
        if std::env::var("PROFILE").as_deref() == Ok("release") {
            println!("cargo:rerun-if-changed=assets/ramdog.manifest");
            res.set_manifest_file("assets/ramdog.manifest");
        }

        res.compile().expect("embed ramdog.ico");
    }
}

# Publicar uma release

O comando do repositório é `./release vX.Y.Z`. Ao pedir `/release` a um agente, use este procedimento. Requer Git, Rust e GitHub CLI autenticado com permissão de escrita no repositório.

1. Compare as mudanças com a última tag publicada e revise o que vai entrar. Não inclua logs, inventários ou configurações pessoais.
2. Atualize a versão em `Cargo.toml` e `Cargo.lock`.
3. Adicione a seção da versão em `CHANGELOG.md` e as patch notes em `docs/releases/vX.Y.Z.md`, com benefícios, correções, instalação e limitações confirmadas.
4. Faça commit dos arquivos e envie `main` ao GitHub. Aguarde o workflow `release` de `main` passar nas cinco plataformas.
5. Execute `./release vX.Y.Z`. O comando verifica versão, notas, branch e árvore limpa, roda testes e envia uma tag anotada.
6. Acompanhe com `gh run list --workflow release.yml --branch vX.Y.Z`, `gh run watch ID` e `gh release view vX.Y.Z`.

O workflow testa e compila Linux x86_64/aarch64, macOS Apple Silicon/Intel e Windows x64. O pacote Linux inclui `ramdog-launch` e a documentação. O Windows inclui o helper térmico, que requer o runtime .NET 8. Os nomes dos pacotes correspondem aos instaladores.

Somente após todas as compilações passarem, o workflow calcula `SHA256SUMS.txt`, cria uma release em rascunho, envia os cinco pacotes e checksums e publica como latest. As notas vêm do arquivo versionado. Uma falha de compilação não publica uma release incompleta; falhas durante o upload deixam um rascunho para nova tentativa.

Se falhar infraestrutura ou upload, use `gh run rerun ID --failed`. Se o código precisar mudar, faça a correção e escolha uma nova versão/tag; não mova uma tag publicada. Uma release já publicada não é sobrescrita por uma reexecução.

Os binários Linux são compilados no Ubuntu 24.04 e exigem glibc 2.39 ou posterior e bibliotecas gráficas compatíveis. Em distribuições anteriores, compile do código na própria distribuição. ARM64 e macOS têm validação de build/testes em CI; suporte específico a hardware e compositor precisa de teste real.

//! Embute o ícone (`assets/agentdeck.ico`) e os dados de versão no executável do Windows.
//! O `.ico` é gerado por `scripts/gerar-icone.ps1` a partir do desenho do logo (`icons.rs`).

fn main() {
    println!("cargo:rerun-if-changed=assets/agentdeck.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/agentdeck.ico");
        res.set("ProductName", "AgentDeck");
        res.set("FileDescription", "AgentDeck");
        if let Err(e) = res.compile() {
            // Sem o compilador de recursos (rc.exe) o app compila igual, só que sem o ícone no .exe.
            println!("cargo:warning=não foi possível embutir o ícone no executável: {e}");
        }
    }
}

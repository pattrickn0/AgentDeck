//! Escopo de escrita de uma tarefa: os caminhos que ela declara que vai mexer.
//!
//! Funções puras, sem git nem disco. O escopo é uma lista de **caminhos** (arquivos e pastas relativos à raiz do
//! repositório), nunca globs: uma pasta reserva tudo o que está dentro dela. Dois escopos se sobrepõem quando um
//! caminho é igual a outro ou é prefixo dele **por componente** (`crates/core` cobre `crates/core/src/git.rs`;
//! `crates/co` não cobre `crates/core`). A comparação ignora maiúsculas e trata `/` e `\` como iguais (o
//! repositório vive no Windows). Quem usa o escopo para pôr tarefas na fila e avisar de arquivo fora dele é
//! `tasks/` (`lifecycle.rs`, `finish.rs`).

/// Caminhos que nenhuma tarefa reserva (ficam fora da conferência de sobreposição e do aviso de "fora do escopo"):
/// toda tarefa mexe neles, e eles já se resolvem na mesclagem (ver `memory/aprendizados/historico-mescla-por-uniao.md`).
/// Cada item vale para si e, sendo pasta, para tudo o que está dentro dela.
pub const UNRESERVED: &[&str] = &["memory", "Cargo.lock"];

/// Quantos caminhos um escopo aceita, e o tamanho de cada um.
pub const MAX_SCOPE_PATHS: usize = 64;
pub const MAX_SCOPE_PATH_CHARS: usize = 260;

/// Os componentes do caminho, em minúsculas, com `/` e `\` equivalentes; `.` e componentes vazios somem.
fn components(path: &str) -> Vec<String> {
    path.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".").map(str::to_lowercase).collect()
}

/// `inner` é igual a `outer` ou está dentro dele (por componente).
fn covers(outer: &[String], inner: &[String]) -> bool {
    inner.len() >= outer.len() && inner[..outer.len()] == *outer
}

/// O caminho é dos que nenhuma tarefa reserva ([`UNRESERVED`])?
pub fn is_unreserved(path: &str) -> bool {
    let parts = components(path);
    UNRESERVED.iter().any(|u| covers(&components(u), &parts))
}

/// Normaliza um caminho do escopo: separador `/`, sem `./` nem barra no fim. Recusa (com o motivo) o que não é um
/// caminho relativo à raiz do repositório: vazio, absoluto, com `..`, com curinga (`*`, `?`) ou longo demais.
pub fn normalize(raw: &str) -> Result<String, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("caminho vazio no escopo".into());
    }
    let shown = || text.chars().take(60).collect::<String>().escape_debug().to_string();
    if text.chars().count() > MAX_SCOPE_PATH_CHARS {
        return Err(format!("caminho longo demais no escopo: \"{}…\"", shown()));
    }
    if text.contains(['*', '?']) {
        return Err(format!("o escopo é uma lista de caminhos, não de padrões (glob): \"{}\"", shown()));
    }
    let windows_drive = text.as_bytes().get(1) == Some(&b':') && text.as_bytes()[0].is_ascii_alphabetic();
    if text.starts_with(['/', '\\']) || windows_drive {
        return Err(format!("o caminho do escopo é relativo à raiz do repositório, não absoluto: \"{}\"", shown()));
    }
    let parts: Vec<&str> = text.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".").collect();
    if parts.is_empty() {
        return Err(format!("caminho inválido no escopo: \"{}\"", shown()));
    }
    if parts.contains(&"..") {
        return Err(format!("o caminho do escopo não pode sair do repositório (..): \"{}\"", shown()));
    }
    Ok(parts.join("/"))
}

/// Normaliza a lista toda, sem repetir caminhos (mesma comparação das sobreposições) e na ordem dada.
pub fn normalize_all<S: AsRef<str>>(raw: &[S]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for r in raw {
        let path = normalize(r.as_ref())?;
        if !out.iter().any(|p| components(p) == components(&path)) {
            out.push(path);
        }
    }
    if out.len() > MAX_SCOPE_PATHS {
        return Err(format!("escopo grande demais: no máximo {MAX_SCOPE_PATHS} caminhos (agrupe em pastas)"));
    }
    Ok(out)
}

/// O escopo sem os caminhos que ninguém reserva: é o que vale para as conferências.
fn reserved(scope: &[String]) -> Vec<Vec<String>> {
    scope.iter().filter(|p| !is_unreserved(p)).map(|p| components(p)).collect()
}

/// Os dois escopos se sobrepõem? Um escopo sem nenhum caminho reservado (vazio, ou só `memory/…`/`Cargo.lock`)
/// não se sobrepõe a nada.
pub fn overlaps(a: &[String], b: &[String]) -> bool {
    let (a, b) = (reserved(a), reserved(b));
    a.iter().any(|x| b.iter().any(|y| covers(x, y) || covers(y, x)))
}

/// Dos arquivos mexidos (caminhos do git, relativos à raiz), os que ficam fora do escopo, sem os que ninguém
/// reserva. Vazio se a tarefa não declarou escopo reservado: aí não há de que se desviar.
pub fn outside(scope: &[String], files: &[String]) -> Vec<String> {
    let scope = reserved(scope);
    if scope.is_empty() {
        return Vec::new();
    }
    files
        .iter()
        .filter(|f| !is_unreserved(f))
        .filter(|f| {
            let parts = components(f);
            !scope.iter().any(|s| covers(s, &parts))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| (*p).to_owned()).collect()
    }

    #[test]
    fn a_folder_covers_what_is_inside_it_by_component() {
        assert!(overlaps(&s(&["crates/core"]), &s(&["crates/core/src/git.rs"])));
        assert!(overlaps(&s(&["crates/core/src/git.rs"]), &s(&["crates/core"])));
        assert!(overlaps(&s(&["crates/core"]), &s(&["crates/core"])));
        assert!(!overlaps(&s(&["crates/co"]), &s(&["crates/core"])));
        assert!(!overlaps(&s(&["crates/core"]), &s(&["crates/core2/x.rs"])));
        assert!(!overlaps(&s(&["crates/app"]), &s(&["crates/cli"])));
    }

    #[test]
    fn comparison_ignores_case_and_separator_kind() {
        assert!(overlaps(&s(&["Crates\\Core"]), &s(&["crates/core/src/git.rs"])));
        assert!(overlaps(&s(&["crates/core/"]), &s(&["./crates//core/src"])));
    }

    #[test]
    fn memory_and_cargo_lock_reserve_nothing() {
        assert!(!overlaps(&s(&["memory/pendencias.md"]), &s(&["memory/pendencias.md"])));
        assert!(!overlaps(&s(&["memory"]), &s(&["memory/GERAL.md"])));
        assert!(!overlaps(&s(&["Cargo.lock"]), &s(&["cargo.lock"])));
        assert!(overlaps(&s(&["crates/app", "memory/pendencias.md"]), &s(&["crates/app/src", "memory/pendencias.md"])));
        assert!(!overlaps(&s(&["crates/app", "memory/pendencias.md"]), &s(&["crates/cli", "memory/pendencias.md"])));
        assert!(is_unreserved("MEMORY/aprendizados/x.md") && !is_unreserved("memory2/x.md") && !is_unreserved("crates/memory"));
    }

    #[test]
    fn an_empty_scope_overlaps_nothing() {
        assert!(!overlaps(&[], &s(&["crates/core"])));
        assert!(!overlaps(&s(&["crates/core"]), &[]));
        assert!(!overlaps(&[], &[]));
    }

    #[test]
    fn normalize_cleans_paths_and_refuses_the_rest() {
        assert_eq!(normalize(" .\\crates\\core\\ ").unwrap(), "crates/core");
        assert_eq!(normalize("crates/core/src/git.rs").unwrap(), "crates/core/src/git.rs");
        for bad in ["", "  ", "/etc/passwd", "\\x", "C:\\x", "c:/x", "../x", "a/../b", "crates/*.rs", "crates/?", ".", "./"] {
            assert!(normalize(bad).is_err(), "{bad:?} devia ser recusado");
        }
        assert!(normalize(&"a/".repeat(200)).is_err());
    }

    #[test]
    fn normalize_all_drops_repeats() {
        assert_eq!(normalize_all(&["crates/core", "Crates\\Core\\", "crates/app"]).unwrap(), s(&["crates/core", "crates/app"]));
        assert!(normalize_all(&["ok", ""]).is_err());
        let many: Vec<String> = (0..MAX_SCOPE_PATHS + 1).map(|i| format!("d{i}")).collect();
        assert!(normalize_all(&many).is_err());
    }

    #[test]
    fn outside_lists_files_the_scope_does_not_cover() {
        let scope = s(&["crates/core/src/tasks.rs", "crates/cli"]);
        let files =
            s(&["crates/core/src/tasks.rs", "crates/cli/src/bin/ask-agent.rs", "crates/core/src/git.rs", "memory/GERAL.md", "Cargo.lock"]);
        assert_eq!(outside(&scope, &files), s(&["crates/core/src/git.rs"]));
        assert!(outside(&[], &files).is_empty(), "sem escopo não há desvio");
        assert!(outside(&s(&["memory/x.md"]), &files).is_empty(), "escopo só de não reservados também não");
    }
}

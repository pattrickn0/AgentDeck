//! Anexos: cópia para `dados/anexos` com prefixo da sessão (equivalente a `attach:*`).

use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

use crate::util::{base36, now_ms, ref_path};

/// Nome seguro para arquivo: remove acentos (NFD sem marcas combinantes) e troca
/// sequências fora de `[A-Za-z0-9_.-]` por `_`. Equivalente a `safeName`.
pub fn safe_name(name: &str) -> String {
    let stripped: String = name.nfd().filter(|c| !('\u{0300}'..='\u{036f}').contains(c)).collect();
    let mut out = String::with_capacity(stripped.len());
    let mut in_bad = false;
    for c in stripped.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
            out.push(c);
            in_bad = false;
        } else if !in_bad {
            out.push('_');
            in_bad = true;
        }
    }
    out
}

/// Pasta de anexos (criada se preciso).
pub(crate) fn attachments_dir(data_dir: &Path) -> std::io::Result<PathBuf> {
    let dir = data_dir.join("anexos");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Cria o destino `s_<sessão>_<ms base36>-<nome>` sem sobrescrever nada: dois anexos com o mesmo
/// nome no mesmo milissegundo (ou nomes que viram o mesmo `safe_name`) ganham um sufixo.
fn create_dest(dir: &Path, session: &str, name: &str) -> std::io::Result<(PathBuf, std::fs::File)> {
    let stamp = base36(now_ms());
    for n in 0u32.. {
        let suffix = if n == 0 { String::new() } else { format!("_{n}") };
        let path = dir.join(format!("s_{session}_{stamp}{suffix}-{name}"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 1000 => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!("o laço só termina retornando")
}

/// Copia arquivos (ignorando pastas e inexistentes) e devolve as referências.
pub(crate) fn attach_files(data_dir: &Path, workspace: &Path, session: &str, files: &[PathBuf]) -> Vec<String> {
    let Ok(dir) = attachments_dir(data_dir) else { return Vec::new() };
    files
        .iter()
        .filter(|src| src.is_file())
        .filter_map(|src| {
            let base = src.file_name()?.to_string_lossy().into_owned();
            let (dest, mut out) = create_dest(&dir, session, &safe_name(&base)).ok()?;
            let copied = std::fs::File::open(src).and_then(|mut f| std::io::copy(&mut f, &mut out));
            if copied.is_err() {
                drop(out);
                let _ = std::fs::remove_file(&dest);
                return None;
            }
            Some(ref_path(workspace, &dest))
        })
        .collect()
}

/// Grava bytes como anexo (equivalente a `attach:saveBuffer`).
pub(crate) fn attach_bytes(data_dir: &Path, workspace: &Path, session: &str, name: &str, bytes: &[u8]) -> anyhow::Result<String> {
    let dir = attachments_dir(data_dir)?;
    let base_name = if !name.is_empty() && name != "image.png" { name.to_owned() } else { format!("imagem-{}.png", now_ms()) };
    // `path.basename(name, ext)`: sem pastas e sem a extensão.
    let file = Path::new(&base_name).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let (stem, ext) = match Path::new(&file).extension() {
        Some(e) => {
            let ext = format!(".{}", e.to_string_lossy());
            (file[..file.len() - ext.len()].to_owned(), ext)
        }
        None => (file.clone(), ".png".to_owned()),
    };
    let (dest, mut out) = create_dest(&dir, session, &format!("{}{ext}", safe_name(&stem)))?;
    std::io::Write::write_all(&mut out, bytes)?;
    Ok(ref_path(workspace, &dest))
}

/// Codifica RGBA 8 bits como PNG.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> anyhow::Result<Vec<u8>> {
    let expected = width as usize * height as usize * 4;
    anyhow::ensure!(rgba.len() == expected, "imagem RGBA com tamanho inválido ({} bytes, esperado {expected})", rgba.len());
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header()?;
        writer.write_image_data(rgba)?;
        writer.finish()?;
    }
    Ok(out)
}

/// Grava uma imagem RGBA (área de transferência) como `screenshot.png`.
pub(crate) fn attach_rgba_png(
    data_dir: &Path,
    workspace: &Path,
    session: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> anyhow::Result<String> {
    let png = encode_png(width, height, rgba)?;
    let dir = attachments_dir(data_dir)?;
    let (dest, mut out) = create_dest(&dir, session, "screenshot.png")?;
    std::io::Write::write_all(&mut out, &png)?;
    Ok(ref_path(workspace, &dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names() {
        assert_eq!(safe_name("Relatório Final (v2).pdf"), "Relatorio_Final_v2_.pdf");
        assert_eq!(safe_name("ação çé ü.txt"), "acao_ce_u.txt");
        assert_eq!(safe_name("a  &&  b"), "a_b");
        assert_eq!(safe_name("já-ok_1.2"), "ja-ok_1.2");
        assert_eq!(safe_name("日本.png"), "_.png");
    }

    #[test]
    fn copies_and_refs() {
        let tmp = tempfile::tempdir().expect("tmp");
        let data = tmp.path().join("dados");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).expect("mkdir");
        let src = tmp.path().join("Olá mundo.txt");
        std::fs::write(&src, "x").expect("write");
        let refs = attach_files(&data, &ws, "sess1", &[src, tmp.path().to_path_buf(), tmp.path().join("nao")]);
        assert_eq!(refs.len(), 1);
        assert!(refs[0].contains("/dados/anexos/s_sess1_"), "{}", refs[0]);
        assert!(refs[0].ends_with("-Ola_mundo.txt"));

        let r = attach_bytes(&data, &ws, "sess1", "Foto é.jpeg", b"img").expect("ok");
        assert!(r.ends_with("-Foto_e.jpeg"), "{r}");
        let r = attach_bytes(&data, &ws, "sess1", "", b"img").expect("ok");
        assert!(r.contains("-imagem-") && r.ends_with(".png"), "{r}");
        let r = attach_bytes(&data, &ws, "sess1", "semext", b"x").expect("ok");
        assert!(r.ends_with("-semext.png"), "{r}");

        // Dentro do workspace a referência é relativa.
        let r = attach_bytes(&ws.join("d"), &ws, "s", "a.txt", b"x").expect("ok");
        assert!(r.starts_with("d/anexos/s_s_"), "{r}");

        // Dois anexos com o mesmo nome em seguida (mesmo milissegundo, em geral) não se sobrescrevem.
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        for (dir, body) in [(&a, "primeiro"), (&b, "segundo")] {
            std::fs::create_dir_all(dir).expect("mkdir");
            std::fs::write(dir.join("foto.txt"), body).expect("write");
        }
        let refs = attach_files(&data, &ws, "dup", &[a.join("foto.txt"), b.join("foto.txt")]);
        assert_eq!(refs.len(), 2);
        assert_ne!(refs[0], refs[1]);
        let anexos = data.join("anexos");
        let mut bodies: Vec<String> = std::fs::read_dir(&anexos)
            .expect("ls")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("s_dup_"))
            .map(|e| std::fs::read_to_string(e.path()).expect("read"))
            .collect();
        bodies.sort();
        assert_eq!(bodies, ["primeiro", "segundo"]);
    }

    #[test]
    fn png_roundtrip() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rgba = [255u8, 0, 0, 255, 0, 255, 0, 128];
        let r = attach_rgba_png(tmp.path(), tmp.path(), "s", 2, 1, &rgba).expect("ok");
        assert!(r.ends_with("-screenshot.png"));
        let bytes = std::fs::read(tmp.path().join(&r)).expect("read");
        let dec = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = dec.read_info().expect("info");
        let mut buf = vec![0; reader.output_buffer_size().expect("size")];
        let info = reader.next_frame(&mut buf).expect("frame");
        assert_eq!((info.width, info.height), (2, 1));
        assert_eq!(&buf[..8], &rgba);
        assert!(encode_png(3, 3, &rgba).is_err());
    }
}

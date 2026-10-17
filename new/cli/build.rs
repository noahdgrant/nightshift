//! Embeds the skills in `../skills` in the binary, so one file carries a CLI and the skills that
//! match it.

use std::fmt::Write;
use std::path::Path;

fn main() {
    let skills = Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills");
    println!("cargo:rerun-if-changed={}", skills.display());
    let mut found: Vec<_> = std::fs::read_dir(&skills)
        .expect("cannot read the skills directory")
        .map(|e| e.expect("cannot read a skill").path())
        .filter(|p| p.join("SKILL.md").is_file())
        .collect();
    found.sort();
    let mut table = String::from("pub const EMBEDDED: &[(&str, &str)] = &[\n");
    for dir in found {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let file = dir.join("SKILL.md").canonicalize().unwrap();
        writeln!(
            table,
            "    ({name:?}, include_str!({:?})),",
            file.display().to_string()
        )
        .unwrap();
    }
    table.push_str("];\n");
    let out = std::env::var("OUT_DIR").unwrap();
    std::fs::write(Path::new(&out).join("skills.rs"), table).unwrap();
}

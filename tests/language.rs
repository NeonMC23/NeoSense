//! Guards against regressions where non-English product strings reappear.

use std::fs;
use std::path::{Path, PathBuf};

const NON_ENGLISH_FRAGMENTS: &[&str] = &[
    "aide",
    "analyse de ",
    "aucun ",
    "capteur",
    "champs insuffisants",
    "chemin invalide",
    "cible",
    "critique",
    "demande utilisateur",
    "demarrage",
    "démarrage",
    "erreur",
    "format texte",
    "hors plage",
    "illisible",
    "introuvable",
    "ligne ",
    "manuel",
    "manquant",
    "mode critique",
    "nom de ventilateur",
    "non bloquant",
    "non fiable",
    "objet attendu",
    "puissance",
    "socket non bloquant",
    "suspendu",
    "tableau fans",
    "température",
    "ventilateur",
    "vitesse",
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read source directory") {
        let path = entry.expect("read source entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn product_source_contains_no_known_non_english_fragments() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    assert!(!files.is_empty());

    let mut failures = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path).expect("read Rust source");
        let lower = text.to_ascii_lowercase();
        for fragment in NON_ENGLISH_FRAGMENTS {
            if lower.contains(fragment) {
                failures.push(format!(
                    "{} contains {:?}",
                    path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(&path)
                        .display(),
                    fragment
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "non-English product fragments:\n{}",
        failures.join("\n")
    );
}

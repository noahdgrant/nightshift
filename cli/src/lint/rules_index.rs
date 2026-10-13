use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use super::relative_links;

/// Checks that `rules/README.md` links `<pack>/README.md` for exactly the `rules/*/` packs.
pub(super) fn check_rules_index(rules: &Path) -> Vec<super::LintError> {
    let index = rules.join("README.md");
    let error = |message: String| super::LintError {
        skill: "rules".into(),
        file: index.to_string_lossy().into_owned(),
        message,
    };
    let packs: BTreeSet<String> = fs::read_dir(rules)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    let Ok(text) = fs::read_to_string(&index) else {
        return if packs.is_empty() {
            Vec::new()
        } else {
            vec![error(
                "missing rules/README.md, the index of the rule packs".into(),
            )]
        };
    };
    let named: BTreeSet<String> = relative_links(&text)
        .iter()
        .filter_map(|l| l.trim_start_matches("./").strip_suffix("/README.md"))
        .filter(|p| !p.contains('/'))
        .map(str::to_string)
        .collect();
    let unnamed = packs
        .difference(&named)
        .map(|p| error(format!("rules/README.md does not name the pack rules/{p}/")));
    let missing = named.difference(&packs).map(|p| {
        error(format!(
            "rules/README.md names rules/{p}/, which does not exist"
        ))
    });
    unnamed.chain(missing).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages(rules: &Path) -> Vec<String> {
        check_rules_index(rules)
            .into_iter()
            .map(|e| e.message)
            .collect()
    }

    #[test]
    fn rules_index_must_name_exactly_the_rule_pack_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let rules = tmp.path().join("rules");
        fs::create_dir_all(&rules).unwrap();
        assert!(messages(&rules).is_empty());

        fs::create_dir_all(rules.join("c")).unwrap();
        fs::create_dir_all(rules.join("rust")).unwrap();
        let errors = check_rules_index(&rules);
        assert_eq!(errors[0].skill, "rules");
        assert_eq!(
            messages(&rules),
            ["missing rules/README.md, the index of the rule packs"]
        );

        let index = "# Rule packs\n\nSee [FACTORY](../docs/FACTORY.md).\n\n\
                     - [`c/`](c/README.md): C.\n- [`go/`](go/README.md): Go.\n";
        fs::write(rules.join("README.md"), index).unwrap();
        assert_eq!(
            messages(&rules),
            [
                "rules/README.md does not name the pack rules/rust/",
                "rules/README.md names rules/go/, which does not exist",
            ]
        );

        fs::create_dir_all(rules.join(".cache")).unwrap();
        fs::write(
            rules.join("README.md"),
            "- [`c/`](c/README.md): C.\n- [`rust/`](./rust/README.md): Rust, \
             tested as [its tests](rust/tests/README.md) say.\n",
        )
        .unwrap();
        assert!(messages(&rules).is_empty());
    }

    #[test]
    fn rules_index_flags_links_to_packs_that_are_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let rules = tmp.path().join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(rules.join("README.md"), "- [`go/`](go/README.md): Go.\n").unwrap();
        assert_eq!(
            messages(&rules),
            ["rules/README.md names rules/go/, which does not exist"]
        );
    }

    #[test]
    fn rules_index_ignores_packs_named_only_in_prose_or_fences() {
        let tmp = tempfile::tempdir().unwrap();
        let rules = tmp.path().join("rules");
        fs::create_dir_all(rules.join("rust")).unwrap();
        fs::write(
            rules.join("README.md"),
            "The `rust/` pack.\n\n```\n[rust](rust/README.md)\n```\n",
        )
        .unwrap();
        assert_eq!(
            messages(&rules),
            ["rules/README.md does not name the pack rules/rust/"]
        );
    }
}

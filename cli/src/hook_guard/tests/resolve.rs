//! Variables the guard resolves before it checks, and what a denial tells the agent.

use super::*;

fn phase() -> Vars {
    Vars::from([
        ("NS_UNIT".to_string(), "u1".to_string()),
        ("NS_WORKTREE".to_string(), "/w".to_string()),
    ])
}

fn judged(cmd: &str, vars: &Vars) -> Option<Denial> {
    judge(&bash_call(cmd), vars)
}

#[test]
fn a_literal_assignment_earlier_in_the_command_is_resolved() {
    for c in [
        "U=build.md; rm .ns/u1/$U",
        "U=build.md\nrm .ns/u1/$U",
        "U=build.md && rm .ns/u1/$U",
        "export U=build.md; rm .ns/u1/$U",
        "U=u1; V=build.md; rm .ns/$U/$V",
        "U=u1 V=build.md; rm .ns/$U/$V",
        "U='build.md'; rm \".ns/u1/$U\"",
        "U=build.md; echo x > .ns/u1/$U",
        "U=build.md; export V; read -r W; rm .ns/u1/$U",
        "U=u1; cp x .ns/$U/history/$F",
        "F=--file; git commit $F=$M",
        "U=u1; cd .ns/$U && rm build.md",
        "U=u1; V=$U; rm .ns/$V/build.md",
        "true && false; U=build.md; rm .ns/u1/$U",
        "U=build.md; (rm .ns/u1/$U)",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn an_assignment_that_may_not_hold_is_not_resolved() {
    for c in [
        "(U=build.md; rm .ns/u1/$U)",
        "(U=build.md); rm .ns/u1/$U",
        "f() { U=build.md; }; f; rm .ns/u1/$U",
        "f() { true; U=build.md; }; rm .ns/u1/$U",
        "function f { U=build.md; }; f; rm .ns/u1/$U",
        "{ U=build.md; }; rm .ns/u1/$U",
        "{ true; U=build.md; }; rm .ns/u1/$U",
        "if true; then U=build.md; fi; rm .ns/u1/$U",
        "if false; then true; U=build.md; fi; rm .ns/u1/$U",
        "for x in; do true; U=build.md; done; rm .ns/u1/$U",
        "while false; do true; U=build.md; done; rm .ns/u1/$U",
        "export$X U=build.md; rm .ns/u1/$U",
        "true && U=build.md; rm .ns/u1/$U",
        "false || U=build.md; rm .ns/u1/$U",
        "U=build.md | cat; rm .ns/u1/$U",
        "U=build.md & rm .ns/u1/$U",
        "U=$(echo build.md); rm .ns/u1/$U",
        "U=\"$X\"; rm .ns/u1/$U",
        "U='a b'; rm .ns/u1/$U",
        "U=*; rm .ns/u1/$U",
        "U=build.md cat x; rm .ns/u1/$U",
        "U=build.md; U=$X; rm .ns/u1/$U",
        "U=build.md; read U; rm .ns/u1/$U",
        "U=build.md; unset U; rm .ns/u1/$U",
        "U=build.md; printf -v U x; rm .ns/u1/$U",
        "U=build.md; read \"$V\"; rm .ns/u1/$U",
        "U=build.md; eval x; rm .ns/u1/$U",
        "U=build.md; source f; rm .ns/u1/$U",
        "U=build.md; . f; rm .ns/u1/$U",
        "U=build.md; echo $(rm .ns/u1/$U)",
        "U=build.md; sh -c 'rm .ns/u1/$U'",
        "U=build.md; export -n U=x; rm .ns/u1/$U",
        "U=build.md; U+=x; rm .ns/u1/$U",
        "U=build.md; U[0]=x; rm .ns/u1/$U",
        "U=build.md; printf -vU x; rm .ns/u1/$U",
        "U=build.md; declare -n R=U; R=x; rm .ns/u1/$U",
        "U=build.md; ((U=5)); rm .ns/u1/$U",
        "U=build.md; echo $((U=5)); rm .ns/u1/$U",
        "U=build.md; echo $[U=5]; rm .ns/u1/$U",
        "U=build.md; echo ${V:=x}; rm .ns/u1/$U",
        "IFS=.; U=build.md; rm .ns/u1/$U",
        "U=build.md; IFS=.; rm .ns/u1/$U",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_resolved_word_is_checked_as_written() {
    for c in [
        "U=u1; rm -r .ns/$U",
        "U=skills; rm -rf .ns/u1/$U",
        "U=.githooks; rm -rf $U/pre-commit",
        "F=--no-verify; git push origin $F",
        "F=--no-verify && git commit $F",
        "export F=-n; git commit $F -m x",
    ] {
        let d = judged(c, &Vars::new()).unwrap_or_else(|| panic!("not denied: {c}"));
        assert!(!d.unread, "{c}: {}", d.why);
    }
}

#[test]
fn ns_unit_and_ns_worktree_resolve_from_the_phase() {
    for c in [
        "echo x > .ns/$NS_UNIT/review-$NS_UNIT.md",
        "cd .ns/u1 && rm -rf $NS_WORKTREE/.ns/u1/history",
    ] {
        assert!(denied(c), "unresolved: {c}");
        assert!(judged(c, &phase()).is_none(), "{c}");
    }
    for c in [
        "rm -rf .ns/$NS_UNIT",
        "rm -rf $NS_WORKTREE/.ns/$NS_UNIT/skills",
    ] {
        let d = judged(c, &phase()).unwrap_or_else(|| panic!("not denied: {c}"));
        assert!(!d.unread, "{c}: {}", d.why);
    }
    assert!(judged("NS_UNIT=$X; rm .ns/u1/review-$NS_UNIT.md", &phase()).is_some());
}

#[test]
fn the_phase_vars_come_from_the_environment_when_literal() {
    let env = |worktree: &'static str| {
        move |name: &str| match name {
            "NS_UNIT" => Some("u1".to_string()),
            "NS_WORKTREE" => Some(worktree.to_string()),
            _ => None,
        }
    };
    assert_eq!(phase_vars(env("/w")), phase());
    let spaced = phase_vars(env("/a b"));
    assert_eq!(spaced.keys().collect::<Vec<_>>(), ["NS_UNIT"]);
    assert!(judged("cd .ns/u1 && rm -rf $NS_WORKTREE/.ns/u1/history", &spaced).is_some());
    assert!(phase_vars(|_| None).is_empty());
}

#[test]
fn an_unreadable_word_asks_for_a_rewrite_and_not_a_stop() {
    for c in [
        "rm .ns/u1/$U",
        "rm -rf .ns/$U/$F",
        "git push $F",
        "git commit $FLAG",
        "$G commit -n",
        "eval \"$S\"",
        "git -C .githooks/$U$V status",
    ] {
        let d = judged(c, &Vars::new()).unwrap_or_else(|| panic!("not denied: {c}"));
        assert!(d.unread, "{c}: {}", d.why);
        let reason = d.reason();
        assert!(!reason.contains("blocked"), "{c}: {reason}");
        assert!(reason.contains("literally"), "{c}: {reason}");
        assert!(reason.contains("Write tool"), "{c}: {reason}");
        assert!(reason.contains(&d.why), "{c}: {reason}");
    }
}

#[test]
fn a_bypass_keeps_its_message() {
    for c in [
        "git commit -n -m x",
        "git push --no-verify",
        "GIT_DIR=/x git commit",
        "rm -r .ns/$U",
        "rm .githooks/pre-commit",
        "rm -rf .ns/$U/skills",
        "git push --no-verify $F",
        "git push {--no-verify,$X}",
    ] {
        let d = judged(c, &Vars::new()).unwrap_or_else(|| panic!("not denied: {c}"));
        assert!(!d.unread, "{c}: {}", d.why);
        assert_eq!(d.reason(), format!("ns: blocked: {}. {RULE}", d.why), "{c}");
    }
    let edit = json!({"tool_name": "Edit", "tool_input": {"file_path": "/r/.githooks/x"}});
    assert!(!judge(&edit, &Vars::new()).unwrap().unread);
}

#[test]
fn an_empty_value_is_not_resolved() {
    for c in [
        "F=; $F git commit -n -m x",
        "F=; $F git push --no-verify",
        "export F=; $F git commit -n",
    ] {
        assert!(denied(c), "{c}");
    }
    let empty = |name: &str| (name == "NS_UNIT").then(String::new);
    assert!(phase_vars(empty).is_empty());
    assert!(judged("$NS_UNIT git commit -n -m x", &phase_vars(empty)).is_some());
}

#[test]
fn a_command_that_may_rewrite_variables_unseen_forgets_them() {
    for c in [
        "F=x; if source f; then :; fi; git commit $F",
        "F=x; time source f; git commit $F",
        "F=x; ! source f; git commit $F",
        "F=x; builtin source f; git commit $F",
        "F=x; command . f; git commit $F",
        "F=x; builtin eval 'F=-n'; git commit $F",
        "F=x; while source f; do :; done; git commit $F",
        "F=x; let 'F = 5'; git commit $F",
        "F=x; trap 'F=-n' DEBUG; git commit $F",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_declaration_or_trap_stops_the_scan_learning_assignments() {
    for c in [
        "F=x; declare -l F; F=-N; git commit $F",
        "typeset -u F; F=-n; git commit $F",
        "local -i F; F=-n; git commit $F",
        "F=x; trap ' F=-n' DEBUG; git commit $F",
        "trap 'F=-n' DEBUG; F=x; git commit $F",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn every_compound_word_stops_the_scan_following_the_command() {
    for w in COMPOUND_WORDS {
        let c = format!("{w} true; U=build.md; rm .ns/u1/$U");
        assert!(denied(&c), "{c}");
    }
}

#[test]
fn every_builtin_that_may_set_a_variable_forgets_it() {
    for s in SETTERS {
        let c = format!("U=build.md; {s} \"$V\"; rm .ns/u1/$U");
        assert!(denied(&c), "{c}");
    }
    for c in [
        "U=build.md; typeset -n R=U; R=x; rm .ns/u1/$U",
        "U=build.md; local U; rm .ns/u1/$U",
        "U=build.md; builtin read U; rm .ns/u1/$U",
        "U=build.md; command read U; rm .ns/u1/$U",
        "U=build.md; builtin read \"$V\"; rm .ns/u1/$U",
        "U=build.md; command read \"$V\"; rm .ns/u1/$U",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn only_a_plain_literal_value_is_resolved() {
    for v in [
        "", "~", "a;b", "$x", "a\\b", "a?", "{a,b}", "a b", "a*", "a[b]", "a&b", "a|b", "a<b",
        "a>b", "a(b)", "a!b", "a#b", "a`b", "a\"b", "a^b",
    ] {
        assert!(!is_literal_value(v), "{v:?}");
    }
    for v in ["a:b@c%d+e,f=g", "--no-verify", "x_y.z/w-1"] {
        assert!(is_literal_value(v), "{v}");
    }
    assert!(judged("F=a:b@c%d+e,f=g; git commit $F", &Vars::new()).is_none());
    for c in [
        "F=~; git commit $F",
        "F='a;b'; git commit $F",
        "F='a?'; git commit $F",
    ] {
        assert!(judged(c, &Vars::new()).is_some_and(|d| d.unread), "{c}");
    }
}

#[test]
fn a_braced_word_with_an_expansion_stays_opaque_after_resolving() {
    let d = judged("F=--no-verify; git push {$F,x}", &Vars::new()).unwrap();
    assert!(!d.unread, "{}", d.why);
    assert!(denied("F=--no-verify; git push $F$X"));
}

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
        "U=build.md; export V; rm .ns/u1/$U",
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
    for s in [
        "printf", "export", "readonly", "unset", "builtin", "command",
    ] {
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

#[test]
fn a_loop_or_function_anywhere_stops_the_scan_following_assignments() {
    for c in [
        "U=zz; while :; do rm -r .git/$U; U=hooks; done",
        "U=zz\nwhile :\ndo\nrm -r .git/$U\nU=hooks\ndone",
        "U=ok; for i in 1 2; do git commit $U; U=-n; done",
        "U=ok; until false; do git commit $U; U=-n; done",
        "U=ok; select i in 1 2; do git commit $U; U=-n; done",
        "U=zz; f(){ rm -r .git/$U; }; U=hooks; f",
        "U=zz\nf(){\nrm -r .git/$U\n}\nU=hooks\nf",
        "U=zz; function f { rm -r .git/$U; }; U=hooks; f",
        "U=ok; git commit $U; f() { :; }",
    ] {
        assert!(denied(c), "{c}");
    }
    assert!(judged("while :; do git commit $NS_UNIT; done", &phase()).is_none());
}

#[test]
fn an_alias_that_assigns_a_variable_forgets_the_variables() {
    assert!(denied("alias g=\"F=--no-verify\"; F=ok; g; git commit $F"));
}

#[test]
fn a_value_with_an_expansion_in_it_is_not_resolved() {
    for c in ["U=build.md$X; rm .ns/u1/$U", "U=${X}build.md; rm .ns/u1/$U"] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_variable_the_shell_rewrites_unseen_is_not_resolved() {
    for c in [
        "_=safe; echo --no-verify; git commit $_ -m x",
        "REPLY=a; read <<< --no-verify; git commit $REPLY -m x",
        "OPTARG=a; getopts \"a:\" o -a--no-verify; git commit $OPTARG -m x",
        "U=ok; mapfile A < f; git commit $U -m x",
        "U=ok; readarray A < f; git commit $U -m x",
        "U=ok; getopts a o; git commit $U -m x",
        "BASH_REMATCH=a; [[ --no-verify =~ (.*) ]]; git commit $BASH_REMATCH -m x",
        "U=ok; [[ x =~ (y) ]]; git commit $U -m x",
        "OPTIND=a; git commit $OPTIND -m x",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_resolved_variable_does_not_make_an_eval_word_look_literal() {
    for c in [
        "U=ok; T=U; eval $T=.git; rm -r $U/hooks",
        "T=U; eval $T=.git; rm -r $U/hooks",
        "T=U; command eval $T=.git; rm -r $U/hooks",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_variable_set_by_code_the_command_defines_is_not_resolved() {
    for c in [
        "eval 'f(){ U=hooks; }'; U=x; f; rm -r .git/$U",
        ". ./defs; U=x; f; rm -r .git/$U",
        "source ./defs; U=x; f; rm -r .git/$U",
        "eval 'trap \"U=hooks\" DEBUG'; U=x; true; rm -r .git/$U",
        "E=eval; $E 'f(){ U=hooks; }'; U=x; f; rm -r .git/$U",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_setter_behind_a_keyword_clears_the_variables() {
    for setter in [
        "read \"$N\" <<< -n",
        "readarray \"$N\" < f",
        "mapfile \"$N\" < f",
        "getopts a \"$N\"",
        "printf -v \"$N\" %s -n",
        "export \"$N=-n\"",
    ] {
        for wrap in [
            "time {}",
            "! {}",
            "if true; then {}; fi",
            "builtin {}",
            "command {}",
        ] {
            let c = format!(
                "F=-q; N=$(echo F); {}; git commit $F",
                wrap.replace("{}", setter)
            );
            assert!(denied(&c), "{c}");
        }
    }
}

#[test]
fn a_readonly_variable_stops_the_scan_learning_assignments() {
    for c in [
        "readonly F=-n; export F=ok; git commit $F -m x",
        "export U=--no-verify; readonly U; U=ok; git commit $U",
        "readonly -p; readonly F=-n; F=ok; git commit $F",
        "export -n F; readonly F=-n; F=ok; git commit $F",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_quoted_or_escaped_name_is_not_an_assignment() {
    for c in [
        "U=--no-verify; \\U=ok; git commit $U",
        "U=--no-verify; \"U\"=ok; git commit $U",
        "U=--no-verify; 'U'=ok; git commit $U",
        "U=--no-verify; U\\=ok; git commit $U",
        "U=--no-verify; V=U; $V=ok; git commit $U",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in ["U=build.md; rm .ns/u1/$U", "U='build.md'; rm .ns/u1/$U"] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn a_current_shell_command_substitution_stops_the_scan_following() {
    for c in [
        "F=ok; echo ${ read F <<< -n; }; git commit $F",
        "F=ok; echo ${| read F <<< -n; }; git commit $F",
        "F=ok; echo ${\tread F <<< -n; }; git commit $F",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn every_shell_managed_variable_but_ifs_and_pwd_is_refused_as_an_assignment() {
    for v in SHELL_MANAGED
        .iter()
        .filter(|v| !["IFS", "PWD", "OLDPWD"].contains(*v))
    {
        let c = format!("{v}=build.md; rm .ns/u1/${v}");
        assert!(denied(&c), "{c}");
    }
}

#[test]
fn a_value_that_doubles_each_statement_is_not_followed() {
    let doubling = "A=x; ".to_string() + &"A=$A$A; ".repeat(40);
    assert!(denied(&format!("{doubling}git commit $A")));
    assert!(denied(&format!("{doubling}rm -r .ns/$A")));
    assert!(!denied(&format!("{doubling}echo $A")));
    let many = "A=x; ".to_string() + &"A=$A$A$A$A; ".repeat(2000);
    assert!(denied(&format!("{many}git commit $A")));
}

#[test]
fn a_value_within_the_cap_is_still_followed() {
    let c = format!("A={}; A=$A$A; rm .ns/u1/$A", "a".repeat(1000));
    assert!(!denied(&c));
    assert!(denied(&format!(
        "A={}; A=$A$A; rm -r .ns/$A",
        "u".repeat(1000)
    )));
}

fn ops_for(cmd: &str) -> usize {
    OPS.with(|c| c.set(0));
    denied(cmd);
    OPS.with(|c| c.get())
}

fn linear(shape: impl Fn(usize) -> String) {
    let (small, large) = (ops_for(&shape(2000)), ops_for(&shape(4000)));
    assert!(small > 0);
    assert!(
        large <= small * 5 / 2,
        "{small} ops at n=2000, {large} at n=4000"
    );
}

#[test]
fn the_work_grows_linearly_with_the_command() {
    linear(|n| format!("echo {}; U=u1; rm -r .ns/$U", words(n)));
    linear(|n| format!("{}; U=u1; rm -r .ns/$U", assigns(n)));
    linear(|n| {
        let dashes = (0..n)
            .map(|i| format!("-r{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{}; read {dashes}; rm .ns/u1/$A0", assigns(n / 4))
    });
    linear(|n| format!("{}U=u1; rm -r .ns/$U", "U=x; echo $U; ".repeat(n)));
    linear(|n| format!("{}{}; rm .ns/u1/$A", "A=b ".repeat(n), ". ".repeat(n)));
    linear(|n| format!("{}echo -{}; rm .ns/u1/$A", "A=b; ".repeat(3), "a".repeat(n)));
}

#[test]
fn a_variable_used_many_times_is_resolved_within_a_byte_budget() {
    let c = format!("A={}; {}", "a".repeat(4000), "echo $A ".repeat(32000));
    assert!(ops_for(&c) < 4 * MAX_RESOLVED + 40 * 32000);
}

#[test]
fn a_command_that_spends_the_resolving_budget_stops_resolving() {
    let big = "a".repeat(4000);
    let uses = |n: usize| {
        format!(
            "A={big}; U={}; {}rm .ns/u1/$U",
            "u".repeat(3000),
            "echo $A; ".repeat(n)
        )
    };
    let (few, many) = (uses(10), uses(200));
    assert!(!denied(&few));
    assert!(denied(&many));
    let flag = format!("A={big}; {}F=-n; git commit $F", "echo $A; ".repeat(200));
    assert!(denied(&flag));
}

#[test]
fn more_learned_variables_than_the_cap_are_all_forgotten() {
    let fill = |n: usize| (0..n).map(|i| format!("V{i}=a; ")).collect::<String>();
    let under = format!("U=build.md; {}rm .ns/u1/$U", fill(MAX_VARS - 1));
    let over = format!("U=build.md; {}rm .ns/u1/$U", fill(MAX_VARS));
    assert!(!denied(&under));
    assert!(denied(&over));
}

#[test]
fn a_name_or_value_over_its_cap_is_not_learned() {
    let name = |n: usize| "N".repeat(n);
    let value = |n: usize| "a".repeat(n);
    let by_name = |n: usize| format!("{0}=build.md; rm .ns/u1/${0}", name(n));
    assert!(!denied(&by_name(MAX_NAME)));
    assert!(denied(&by_name(MAX_NAME + 1)));
    let by_value = |n: usize| format!("A=build.md; A={}; rm .ns/u1/$A", value(n));
    assert!(denied(&by_value(MAX_VALUE + 1)));
    assert!(denied(&format!("A={}; rm .ns/u1/$A", value(MAX_VALUE))));
}

#[test]
fn a_dash_word_still_forgets_the_variable_it_ends_with() {
    for c in [
        "U=build.md; read -rU; rm .ns/u1/$U",
        "F=ok; printf -vF -- -n; git commit $F",
        "F=ok; read -r F <<< -n; git commit $F",
    ] {
        assert!(denied(c), "{c}");
    }
}

fn words(n: usize) -> String {
    (0..n)
        .map(|i| format!("w{i}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn assigns(n: usize) -> String {
    (0..n)
        .map(|i| format!("V{i}=a{i}"))
        .collect::<Vec<_>>()
        .join("; ")
}

#[test]
fn a_heredoc_body_that_assigns_stops_the_scan_following() {
    for c in [
        "N=-q; : <<EOF\n$((N=7))\nEOF\ngit commit $N",
        "N=-q; : <<EOF\n${ N=--no-verify; }\nEOF\ngit commit $N",
        "N=-q; : <<-EOF\n\t$((N=7))\n\tEOF\ngit commit $N",
        "N=-q; : <<EOF\n`echo`\nEOF\ngit commit $N",
    ] {
        assert!(denied(c), "{c}");
    }
    for c in [
        "N=-q; : <<'EOF'\n$((N=7))\nEOF\ngit commit $N",
        "U=build.md; cat <<'EOF'\n$((N=7))\nEOF\nrm .ns/u1/$U",
        "U=build.md; cat <<EOF\nplain\nEOF\nrm .ns/u1/$U",
    ] {
        assert!(!denied(c), "{c}");
    }
}

#[test]
fn a_name_or_dot_neighbour_of_an_assignment_keeps_the_verdict() {
    for c in [
        "F=build.md; rm .ns/u1/${F}",
        "F=build.md; cd . && rm .ns/u1/$F",
    ] {
        assert!(!denied(c), "{c}");
    }
    for c in [
        "U=build.md; (. f); rm .ns/u1/$U",
        "U=build.md; . f; rm .ns/u1/$U",
    ] {
        assert!(denied(c), "{c}");
    }
}

#[test]
fn a_long_command_gives_the_same_verdict() {
    let words = |n: usize| {
        (0..n)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let assigns = |n: usize| {
        (0..n)
            .map(|i| format!("V{i}=a{i}"))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let n = 2000;
    assert!(!denied(&format!(
        "echo {}; U=build.md; rm .ns/u1/$U",
        words(n)
    )));
    assert!(denied(&format!("echo {}; U=u1; rm -r .ns/$U", words(n))));
    assert!(!denied(&format!(
        "{}; U=build.md; rm .ns/u1/$U",
        assigns(n)
    )));
    assert!(denied(&format!("{}; U=u1; rm -r .ns/$U", assigns(n))));
    let uses = "U=x; echo $U; ".repeat(n);
    assert!(denied(&format!("{uses}U=u1; rm -r .ns/$U")));
}

#[test]
fn a_dot_that_is_not_the_source_builtin_does_not_stop_resolving() {
    assert!(!denied("U=build.md; git add .; rm .ns/u1/$U"));
    assert!(denied("U=build.md; . f; rm .ns/u1/$U"));
}

#[test]
fn a_value_over_the_cap_costs_no_copy() {
    let c = format!(
        "{}rm .ns/u1/$A",
        format!("A={}; ", "a".repeat(MAX_VALUE + 1)).repeat(500)
    );
    assert!(ops_for(&c) < 100 * 500);
}

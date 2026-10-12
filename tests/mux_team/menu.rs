// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

use super::*;

/// The native half of `scripts/screenshots.sh` (`scripts/shots-ctl.sh`): what the script asks of the multiplexer,
/// answered for a team of fakes, without vhs, a window or a real Claude. C1.
#[test]
#[ignore = "needs bash, jq and python3; run with --ignored"]
fn screenshots_script_asks_of_the_multiplexer() {
    let mut team = Team::new("shots", &tables("shots", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let mut term = client(&team, "lead");
    let lib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/shots-ctl.sh");
    let prelude = format!(
        "set -euo pipefail; source '{}'; session=$TEAM_NAME; state=$TEAM_STATE; \
         OWN_RECRUIT=$RECRUIT; ",
        lib.display()
    );
    let sh = |script: &str| {
        let (ok, text) = team.bash(&format!("{prelude}{script}"));
        assert!(ok, "{script}\n{text}");
        text.trim().to_string()
    };
    let mut checks = Checks::default();
    for role in ["lead", "dev", "dashboard", "journal"] {
        let id = sh(&format!("pane_of {role}"));
        checks.check(&format!("pane_of {role}"), id.starts_with('p'), || id.clone());
    }
    let members = sh("member_panes | wc -l | tr -d ' '");
    checks.check("member_panes counts the members only", members == "2", || members.clone());
    let all = sh("shown_panes | wc -l | tr -d ' '");
    checks.check("shown_panes counts the panels too", all == "4", || all.clone());
    let text = sh("pane_text \"$(pane_of lead)\"");
    checks.check("pane_text", text.contains("FAKE CLAUDE lead"), || text.clone());
    checks.check("state_of_team", sh("state_of_team") == team.state().to_string_lossy(), || sh("state_of_team"));
    checks.check("has_second_tab", sh("has_second_tab && echo yes") == "yes", String::new);
    checks.check("attach_command names the team", sh("attach_command").ends_with("attach shots"), || {
        sh("attach_command")
    });
    // A client is attached: the tabs and the menu, by `_ctl key`.
    checks.check("has_client", sh("has_client && echo yes") == "yes", String::new);
    sh("select_second");
    let second = wait(Duration::from_secs(3), || has_header(&term, "dev"));
    checks.check("select_second", second, || shown(&term));
    sh("select_first");
    let first = wait(Duration::from_secs(3), || has_header(&term, "lead"));
    checks.check("select_first", first, || shown(&term));
    sh("open_menu");
    let menu = wait(Duration::from_secs(8), || shown(&term).contains("Nouvel agent"));
    checks.check("open_menu", menu, || shown(&term));
    close_menu(&team, &mut term);
    // The focus goes to a member of the second tab, by ⌥n from the first.
    sh("select_second");
    wait(Duration::from_secs(3), || has_header(&term, "dev"));
    sh("focus_member dev");
    checks.check("focus_member", wait(Duration::from_secs(3), || active(&term, &["dev"]) == ["dev"]), || shown(&term));
    sh("select_first");
    wait(Duration::from_secs(3), || has_header(&term, "lead"));
    // The header and the card of each member say the same state: told by the fakes (OSC 7501 and `claude agents`).
    says(&team, "lead", "working");
    team.ctl(["send", "--pane", "lead", "agent busy\\r"]);
    says(&team, "dev", "idle");
    wait(Duration::from_secs(6), || {
        let (ok, _) = team.bash(&format!("{prelude}states_agree"));
        ok
    });
    let (agree, why) = team.bash(&format!("{prelude}states_agree"));
    checks.check("states_agree: header and card alike", agree, || why.clone());
    // And apart. The team can no longer be made to disagree (a pane that says its state by OSC 7501 gives the card
    // that state, whatever `claude agents` says), so the check is given a screen where they do: the header turns, the
    // card rests.
    let (apart, said) = team.bash(&format!(
        "{prelude}ctl() {{ printf '%s\\n' '┏━ ⠋ lead ━━━━━━━━━ 11s ━┓' '┃  ┃│╻ ◷ lead     11s │'; }}; states_agree"
    ));
    checks.check("states_agree: tells them apart", !apart && said.contains("lead (header work, card rest)"), || {
        said.clone()
    });
    // A text typed as a user would reach the pane's program: `osc working` makes the fake say it works.
    sh("pane_type \"$(pane_of lead)\" 'osc working'");
    let working = wait(Duration::from_secs(4), || {
        let header = term.screen().into_iter().find(|r| r.contains(" lead ") && r.contains('┏'));
        header.is_some_and(|h| h.chars().any(|c| ('\u{2800}'..='\u{28ff}').contains(&c)))
    });
    checks.check("pane_type", working, || shown(&term));
    // The crop of the dashboard: a frame at the right of the first tab.
    let rect = sh("pane_rect \"$(pane_of dashboard)\" 'Tableau de bord'");
    let numbers: Vec<usize> = rect.split_whitespace().filter_map(|n| n.parse().ok()).collect();
    checks.check(
        "pane_rect finds the dashboard's frame",
        numbers.len() == 4 && numbers[0] > 20 && numbers[2] > 20 && numbers[3] > 10,
        || rect.clone(),
    );
    drop(term);
    sh("stop_team");
    let stopped = wait(Duration::from_secs(5), || !team.running());
    checks.check("stop_team", stopped, String::new);
    checks.done();
    team.stopped_by_the_user();
}

/// L1: with team files that no longer read, the menu is read-only (members as launched, a message naming the file),
/// offers only « Détacher » and « Quitter », and « Quitter » still stops the team.
#[test]
fn unreadable_files_open_a_read_only_menu() {
    let mut team = Team::new("ro", &tables("ro", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let mut term = client(&team, "lead");
    std::fs::write(team.proj.join(".recruit/settings.toml"), "this is = not [valid toml\n").expect("break the file");
    let keys = alt(&term, 'r');
    term.send(&keys);
    let opened = wait(Duration::from_secs(8), || shown(&term).contains("lecture seule"));
    let mut checks = Checks::default();
    checks.check("L1 the menu says it is read-only", opened, || shown(&term));
    let text = shown(&term);
    checks.check("L1 the members as launched", text.contains("lead") && text.contains("dev"), || text.clone());
    checks.check("L1 it names the file", text.contains("settings.toml") && text.contains("TOML"), || text.clone());
    checks.check(
        "L1 Détacher and Quitter only",
        text.contains("d Détacher")
            && text.contains("q Quitter")
            && !text.contains("Nouvel agent")
            && !text.contains("Réinitialiser"),
        || text.clone(),
    );
    term.send(b"\x1b");
    let closed = wait(Duration::from_secs(10), || !shown(&term).contains("lecture seule"));
    checks.check("L1 Esc closes it", closed, || shown(&term));
    let keys = alt(&term, 'r');
    term.send(&keys);
    wait(Duration::from_secs(8), || shown(&term).contains("lecture seule"));
    term.send(b"q");
    let asked = wait(Duration::from_secs(10), || shown(&term).contains("Quitter l'équipe ?"));
    checks.check("L1 « q » asks to quit the team", asked, || shown(&term));
    let tmux_said = shown(&term).contains("tmux");
    checks.check("L1 the question does not speak of tmux", !tmux_said, || shown(&term));
    // It opens on Annuler: back to Quitter, then Enter.
    term.send(b"\x1b[D");
    std::thread::sleep(Duration::from_millis(300));
    term.send(b"\r");
    let stopped = wait(Duration::from_secs(8), || !team.running());
    checks.check("L1 Quitter stops the team", stopped, || shown(&term));
    drop(term);
    checks.done();
    team.stopped_by_the_user();
}

/// What the mod hands over at each tick (`mod/hooks/register.ts`, `tick`: `session`, `model`, `own`, `context`,
/// `rateLimits`, `threshold`, `compacted`, `agents`) is kept in the member's report (`<state>/members/<member>.json`),
/// the `compacted: false` it always carries included. Found by a real session on 2026-10-11: with `compacted` a flag
/// in the input and a compaction in the report, the whole input failed to read, and every report came back empty (no
/// model, no context, nothing for the compaction to measure).
#[test]
fn a_tick_from_the_mod_is_kept() {
    use std::io::Write;
    let mut team = Team::new("tick", &tables("tick", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let state = team.state();
    let input = serde_json::json!({
        "session": "11111111-2222-3333-4444-555555555555",
        "model": "claude-haiku-5-5",
        "own": {"model": "claude-haiku-5-5", "effort": "high"},
        "context": {"tokens": 24000, "window": 200000, "percent": 12.0},
        "rateLimits": [{"kind": "five_hour", "percentUsed": 21.0}],
        "threshold": {"tokens": 167000, "enabled": true, "window": 200000, "model": "claude-haiku-5-5", "used": 23000},
        "compacted": false,
        "agents": [],
    });
    let mut child = team
        .command()
        .args(["_mod", "tick"])
        .arg(&state)
        .arg("lead")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("_mod tick");
    child.stdin.take().expect("stdin").write_all(input.to_string().as_bytes()).expect("input");
    let out = child.wait_with_output().expect("tick");
    assert!(out.status.success(), "_mod tick: {}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(state.join("members/lead.json")).expect("the report the tick wrote"),
    )
    .expect("report: JSON");
    let mut checks = Checks::default();
    checks.check("a tick keeps the session", report["session"] == "11111111-2222-3333-4444-555555555555", || {
        report.to_string()
    });
    checks.check("a tick keeps the model", report["model"] == "claude-haiku-5-5", || report.to_string());
    checks.check("a tick keeps the context", report["context"]["percent"] == 12.0, || report.to_string());
    checks.check("a tick keeps the threshold", report["threshold"]["tokens"] == 167000, || report.to_string());
    checks.check("a tick keeps the limits", report["rateLimits"].as_array().is_some_and(|l| l.len() == 1), || {
        report.to_string()
    });
    checks.done();
    team.stop();
}

/// R2: `/recruit` (the mod calls `recruit _mod command <state> <member> recruit`): with a client, opens the same
/// menu and says so; with the menu already open, says so; with no client, says how to get one. In words that name no
/// tmux.
#[test]
fn slash_recruit_opens_the_menu() {
    let mut team = Team::new("slash", &tables("slash", &["lead"], &[("dev", None)], ""));
    team.launch(&["lead", "dev"]);
    let state = team.state();
    let slash = |team: &Team| {
        let out = team.recruit([
            std::ffi::OsStr::new("_mod"),
            std::ffi::OsStr::new("command"),
            state.as_os_str(),
            std::ffi::OsStr::new("lead"),
            std::ffi::OsStr::new("recruit"),
        ]);
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    };
    let mut checks = Checks::default();
    // No client yet.
    let none = slash(&team);
    checks.check(
        "R2 no client: says how to get one",
        none.contains("recruit attach") && !none.contains("tmux"),
        || none.clone(),
    );
    let mut term = client(&team, "lead");
    let first = slash(&team);
    let open = wait(Duration::from_secs(8), || shown(&term).contains("Nouvel agent"));
    checks
        .check("R2 /recruit opens the menu", open && first.contains("ouvert"), || format!("{first}\n{}", shown(&term)));
    checks.check("R2 it names no tmux", !first.contains("tmux"), || first.clone());
    let again = slash(&team);
    checks.check("R2 already open: says so", again.contains("déjà ouvert") && !again.contains("tmux"), || {
        again.clone()
    });
    term.send(b"\x1b");
    let closed = wait(Duration::from_secs(5), || menu_text(&team).is_none());
    checks.check("R2 the menu closes", closed, || shown(&term));
    drop(term);
    checks.done();
    team.stop();
}

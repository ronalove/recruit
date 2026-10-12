// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour

use std::collections::HashMap;

use crate::config::{self, Edit, Field, Member, Origin, Source};
use crate::ui::look::{self, State};
use crate::{i18n, t};

use super::fixture::{Fake, layered, seen, team};
use super::text::{first_line, instructions_count, source_name};
use super::*;

fn quiet() -> Fake {
    Fake::quiet()
}

fn sheet() -> Sheet {
    seen(team(&[("coordinateur", true), ("dev", false), ("ops", false)]))
}

fn keys(sheet: &mut Sheet, keys: &[Key], env: &dyn Env) -> Vec<Effect> {
    let mut effects = Vec::new();
    for key in keys {
        effects = sheet.key(key.clone(), env);
    }
    effects
}

fn typed(sheet: &mut Sheet, text: &str) {
    for c in text.chars() {
        sheet.key(Key::Char(c), &quiet());
    }
}

fn to_row(sheet: &mut Sheet, fid: Fid) {
    sheet.pane = Pane::Sheet;
    sheet.row = sheet.focusable().iter().position(|f| *f == fid).expect("the row");
}

fn confirm(sheet: &Sheet) -> &Confirm {
    let Overlay::Confirm(confirm) = &sheet.overlay else { panic!("no dialog: {:?}", sheet.overlay) };
    confirm
}

/// `recruit _menu --member m --field f`: the member's sheet, its field focused; an unknown member or field left out.
#[test]
fn opened_on_a_member_and_a_field() {
    let mut s = sheet();
    s.open_on("dev", Some(Fid::Effort));
    assert_eq!((s.entry.clone(), s.pane, s.field()), (Entry::Member("dev".into()), Pane::Sheet, Some(Fid::Effort)));
    s.open_on("dev", Some(Fid::Name));
    assert_eq!(s.field(), Some(Fid::Name));
    // No field: the member chosen in the list.
    let mut s = sheet();
    s.open_on("dev", None);
    assert_eq!((s.entry.clone(), s.pane), (Entry::Member("dev".into()), Pane::List));
    // An unknown member: as without.
    let mut s = sheet();
    s.open_on("nobody", Some(Fid::Model));
    assert_eq!((s.entry.clone(), s.pane), (Entry::Member("coordinateur".into()), Pane::List));
}

/// A member whose turn failed: « Reprendre » first on its sheet, chosen when the menu opens on it;
/// ⏎ asks its mod once, then says what it did.
#[test]
fn resuming_a_failed_turn() {
    let mut sheet = sheet();
    sheet.states.insert("dev".into(), (State::Error, 100));
    sheet.open_on("dev", None);
    assert_eq!((sheet.pane, sheet.field()), (Pane::Sheet, Some(Fid::Resume)));
    assert_eq!(sheet.value(Fid::Resume), t!("lui envoyer « continue »", "send it “continue”"));
    assert!(sheet.hint().is_some_and(|hint| hint.contains("continue")));
    assert_eq!(sheet.key(Key::Enter, &quiet()), [Effect::Resume("dev".into())]);
    // Asked: not again until its mod answers.
    assert!(sheet.key(Key::Enter, &quiet()).is_empty());
    assert_eq!(sheet.value(Fid::Resume), t!("envoi de « continue »…", "sending “continue”…"));
    sheet.resumed("dev", Resuming::Draft);
    assert_eq!(
        sheet.value(Fid::Resume),
        t!("pas envoyé : sa saisie contient du texte", "not sent: its prompt holds a draft")
    );
    sheet.resumed("ops", Resuming::Sent);
    assert_eq!(sheet.resuming, Some(("dev".into(), Resuming::Draft)));
    assert_eq!(sheet.key(Key::Enter, &quiet()), [Effect::Resume("dev".into())]);
    sheet.resumed("dev", Resuming::Sent);
    assert_eq!(sheet.value(Fid::Resume), t!("« continue » envoyé", "“continue” sent"));
    // A field asked for comes first; another member has no such row.
    sheet.open_on("dev", Some(Fid::Model));
    assert_eq!(sheet.field(), Some(Fid::Model));
    sheet.open_on("ops", None);
    assert_eq!(sheet.pane, Pane::List);
    assert!(!sheet.focusable().contains(&Fid::Resume));
    // Back at work: the row goes.
    sheet.states.insert("dev".into(), (State::Working, 200));
    sheet.choose(Entry::Member("dev".into()));
    assert!(!sheet.focusable().contains(&Fid::Resume));
}

#[test]
fn the_list_then_the_sheet_then_out() {
    let mut s = sheet();
    assert_eq!(s.entry, Entry::Member("coordinateur".into()), "the contacts first");
    assert_eq!(s.entries().last(), Some(&Entry::New));
    keys(&mut s, &[Key::Down], &quiet());
    assert_eq!(s.entry, Entry::Member("dev".into()));
    keys(&mut s, &[Key::Enter], &quiet());
    assert_eq!((s.pane, s.field()), (Pane::Sheet, Some(Fid::Model)));
    assert!(s.focusable().contains(&Fid::Tab), "an agent has its tab");
    keys(&mut s, &[Key::Esc, Key::Up], &quiet());
    assert!(!s.focusable().contains(&Fid::Tab), "a contact is in the first tab");
    assert_eq!(keys(&mut s, &[Key::Esc], &quiet()), [Effect::Quit]);
}

#[test]
fn an_inherited_value_is_the_default() {
    let mut s = sheet();
    to_row(&mut s, Fid::Model);
    let labels: Vec<String> = s.choices(Fid::Model, false).into_iter().map(|c| c.label).collect();
    assert_eq!(labels[1..], ["Sonnet", "Haiku", "Fable"], "Opus is the default's, not listed apart");
    assert_eq!(s.current(Fid::Model), Some(0));
    let effects = keys(&mut s, &[Key::Right], &quiet());
    let [Effect::Apply { edits, said: Done::Text(said), .. }] = effects.as_slice() else { panic!("{effects:?}") };
    assert_eq!(edits, &[Edit::Set { member: "coordinateur".into(), field: Field::Model(Some("sonnet".into())) }]);
    assert!(said.ends_with(&t!(" : effet dès sa prochaine requête.", ": in effect from its next request.")));
    let effects = keys(&mut s, &[Key::Left], &quiet());
    assert!(
        matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::Set { member: "coordinateur".into(), field: Field::Model(Some("fable".into())) }]),
        "round to the last"
    );
    // Written by the member as the team's: still the default.
    s.team.people[0].layers.as_mut().unwrap().model =
        layered(Some("opus"), Source::Personal, Some((Some("opus"), Source::Team)));
    assert_eq!(s.current(Fid::Model), Some(0));
    // A mode prepared back to the inherited one: nothing prepared, nothing said.
    let mut s = sheet();
    s.team.people[0].layers.as_mut().unwrap().permission_mode =
        layered(Some("auto"), Source::Team, Some((Some("auto"), Source::Team)));
    to_row(&mut s, Fid::Mode);
    assert!(!s.choices(Fid::Mode, false).iter().any(|c| c.value.as_deref() == Some("auto")));
    keys(&mut s, &[Key::Right, Key::Left], &quiet());
    assert_eq!(s.pending, None);
    assert_eq!(
        Setting::Mode.changed("x", false),
        t!("Mode de permission de « x » changé.", "Permission mode of \"x\" changed.")
    );
}

#[test]
fn a_change_that_restarts_waits_for_enter() {
    let mut s = sheet();
    let env = Fake::restarting(&["coordinateur"]);
    to_row(&mut s, Fid::Effort);
    assert!(keys(&mut s, &[Key::Right], &env).is_empty(), "prepared, not written");
    assert_eq!(s.value(Fid::Effort), look::effort_label("low"));
    keys(&mut s, &[Key::Right], &env);
    assert_eq!(s.pending, Some((Fid::Effort, 2)));
    // Another row: given up.
    keys(&mut s, &[Key::Down, Key::Up], &env);
    assert!(s.pending.is_none());
    keys(&mut s, &[Key::Left], &env);
    assert_eq!(s.value(Fid::Effort), look::effort_label("max"), "round from the default");
    let effects = keys(&mut s, &[Key::Enter], &env);
    let [Effect::Apply { said: Done::Text(said), .. }] = effects.as_slice() else { panic!("{effects:?}") };
    assert!(said.ends_with(&t!(" : relancé sur sa conversation.", ": restarted on its conversation.")));
    // Esc gives up a value prepared, and stays on the sheet.
    to_row(&mut s, Fid::Mode);
    keys(&mut s, &[Key::Right, Key::Esc], &env);
    assert_eq!((s.pending, s.pane), (None, Pane::Sheet));
}

#[test]
fn a_value_prepared_is_given_up() {
    let env = Fake::restarting(&["dev"]);
    let mut s = sheet();
    s.choose(Entry::Member("dev".into()));
    let prepare = |s: &mut Sheet| {
        to_row(s, Fid::Mode);
        keys(s, &[Key::Right], &env);
        assert!(s.pending.is_some());
    };
    prepare(&mut s);
    s.click(Target::Entry(Entry::Member("dev".into())), &env);
    assert!(s.pending.is_none(), "a click on its own card");
    prepare(&mut s);
    s.wheel(true, false, &env);
    assert!(s.pending.is_none(), "the wheel");
    prepare(&mut s);
    s.click(Target::Restart(false), &env);
    assert!(s.pending.is_none(), "a click on « restart »");
}

#[test]
fn a_member_at_work_is_asked_first() {
    let mut s = sheet();
    s.states.insert("coordinateur".into(), (State::Working, 0));
    let env = Fake::restarting(&["coordinateur"]);
    to_row(&mut s, Fid::Mode);
    keys(&mut s, &[Key::Right], &env);
    assert!(keys(&mut s, &[Key::Enter], &env).is_empty());
    assert!(matches!(&confirm(&s).busy[..], [(State::Working, said)] if said.contains("coordinateur")));
    assert_eq!(confirm(&s).at, 1, "« cancel » at first");
    assert!(keys(&mut s, &[Key::Enter], &env).is_empty(), "⏎ cancels");
    assert!(matches!(s.overlay, Overlay::None));
    keys(&mut s, &[Key::Right, Key::Enter], &env);
    let effects = keys(&mut s, &[Key::Left, Key::Enter], &env);
    assert!(matches!(effects.as_slice(), [Effect::Apply { .. }]), "← then ⏎");
}

#[test]
fn one_waiting_for_the_user_is_said_apart() {
    let mut s = sheet();
    s.states.insert("dev".into(), (State::Waiting, 0));
    s.states.insert("ops".into(), (State::Working, 0));
    s.choose(Entry::Member("dev".into()));
    to_row(&mut s, Fid::Restart);
    keys(&mut s, &[Key::Enter], &quiet());
    assert!(matches!(&confirm(&s).busy[..], [(State::Waiting, said)] if said.contains("dev")));
    assert_ne!(confirm(&s).busy[0].1, s.interrupted(&["ops".into()])[0].1, "not the sentence for those at work");
    // Everyone: those at work, then those waiting.
    s.overlay = Overlay::None;
    s.pane = Pane::List;
    keys(&mut s, &[Key::Char('R')], &quiet());
    assert_eq!(confirm(&s).busy.iter().map(|(state, _)| *state).collect::<Vec<_>>(), [State::Working, State::Waiting]);
}

#[test]
fn unknown_states_may_be_at_work() {
    let mut s = sheet();
    s.seen(None);
    s.choose(Entry::Member("dev".into()));
    to_row(&mut s, Fid::Restart);
    assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty(), "asked");
    assert!(matches!(&confirm(&s).busy[..], [(State::Other, _)]));
    // Seen, and no session: nobody to interrupt.
    s.overlay = Overlay::None;
    s.seen(Some(HashMap::new()));
    assert!(matches!(&keys(&mut s, &[Key::Enter], &quiet())[..], [Effect::Restart { .. }]));
}

#[test]
fn dialogs_cancel_at_first() {
    for open in [Key::Char(Action::Stop.key()), Key::Char('R')] {
        let mut s = sheet();
        keys(&mut s, std::slice::from_ref(&open), &quiet());
        assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty(), "⏎ cancels: {open:?}");
        keys(&mut s, std::slice::from_ref(&open), &quiet());
        assert!(keys(&mut s, &[Key::Esc], &quiet()).is_empty());
        keys(&mut s, std::slice::from_ref(&open), &quiet());
        assert_eq!(keys(&mut s, &[Key::Tab, Key::Enter], &quiet()).len(), 1, "Tab to the action");
    }
    let mut s = sheet();
    s.choose(Entry::Member("dev".into()));
    to_row(&mut s, Fid::Remove);
    keys(&mut s, &[Key::Enter], &quiet());
    assert!(confirm(&s).danger);
    assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty(), "Retirer ⏎ ⏎ removes nothing");
}

#[test]
fn the_only_contact_needs_another() {
    let mut s = sheet();
    to_row(&mut s, Fid::Contact);
    keys(&mut s, &[Key::Right], &quiet());
    assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty());
    let Overlay::List(list) = &s.overlay else { panic!("no list") };
    assert_eq!(list.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["dev", "ops"]);
    let effects = keys(&mut s, &[Key::Down, Key::Enter], &quiet());
    let [Effect::Apply { edits, said: Done::Contacts, .. }] = effects.as_slice() else { panic!("{effects:?}") };
    assert_eq!(
        edits,
        &[
            Edit::Set { member: "coordinateur".into(), field: Field::Contact(false) },
            Edit::Set { member: "ops".into(), field: Field::Contact(true) },
        ]
    );
    // Alone in the team: it stays the contact.
    let mut s = seen(team(&[("seul", true)]));
    to_row(&mut s, Fid::Contact);
    keys(&mut s, &[Key::Right, Key::Enter], &quiet());
    assert!(matches!(s.overlay, Overlay::None), "no empty list");
    assert!(matches!(s.said, Some(Said::Failed(_))));
}

#[test]
fn two_contacts_at_most() {
    let mut s = seen(team(&[("a", true), ("b", true), ("c", false)]));
    s.choose(Entry::Member("c".into()));
    to_row(&mut s, Fid::Contact);
    keys(&mut s, &[Key::Left], &quiet());
    assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty());
    assert!(matches!(&s.said, Some(Said::Failed(said)) if said.contains(&t!("ou", "or"))));
}

#[test]
fn removing() {
    let mut s = seen(team(&[("seul", true)]));
    to_row(&mut s, Fid::Remove);
    keys(&mut s, &[Key::Enter], &quiet());
    assert!(matches!(s.said, Some(Said::Failed(_))), "not the last one");

    let mut s = sheet();
    to_row(&mut s, Fid::Remove);
    keys(&mut s, &[Key::Enter], &quiet());
    let Overlay::List(_) = &s.overlay else { panic!("who takes its place") };
    keys(&mut s, &[Key::Enter], &quiet());
    assert!(confirm(&s).danger);
    let [Effect::Apply { edits, .. }] = confirm(&s).then.as_slice() else { panic!() };
    assert_eq!(
        edits,
        &[Edit::Set { member: "dev".into(), field: Field::Contact(true) }, Edit::Remove("coordinateur".into())]
    );
    assert_eq!(keys(&mut s, &[Key::Left, Key::Enter], &quiet()).len(), 1);
}

#[test]
fn renaming_checks_as_it_is_typed() {
    let mut s = sheet();
    s.choose(Entry::Member("ops".into()));
    to_row(&mut s, Fid::Name);
    keys(&mut s, &[Key::Enter, Key::EraseAll], &quiet());
    typed(&mut s, "Dev");
    let Overlay::Typing(typing) = &s.overlay else { panic!("not typing") };
    assert!(typing.error.is_some(), "differs by case only");
    assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty(), "refused");
    keys(&mut s, &[Key::Backspace, Key::Backspace, Key::Backspace], &quiet());
    typed(&mut s, "release");
    let effects = keys(&mut s, &[Key::Enter], &quiet());
    let [Effect::Apply { edits, select, said: Done::Text(said) }] = effects.as_slice() else { panic!("{effects:?}") };
    assert_eq!(edits, &[Edit::Rename { from: "ops".into(), to: "release".into() }]);
    assert_eq!(select.as_deref(), Some("release"));
    assert!(said.ends_with(&t!(" : relancé sur sa conversation.", ": restarted on its conversation.")));
    // Nothing exchanged yet: a new conversation.
    let mut s = sheet();
    to_row(&mut s, Fid::Name);
    keys(&mut s, &[Key::Enter, Key::EraseAll], &quiet());
    typed(&mut s, "chef");
    let fresh = Fake { conversation: false, ..Fake::quiet() };
    let effects = keys(&mut s, &[Key::Enter], &fresh);
    assert!(
        matches!(&effects[..], [Effect::Apply { said: Done::Text(said), .. }] if said.contains(&t!("conversation neuve", "new conversation")))
    );
}

#[test]
fn typing_in_place() {
    let mut s = sheet();
    to_row(&mut s, Fid::Role);
    keys(&mut s, &[Key::Enter, Key::Home, Key::Char('«'), Key::End, Key::EraseWord], &quiet());
    let Overlay::Typing(typing) = &s.overlay else { panic!("not typing") };
    assert_eq!(typing.text, "«Rôle de ");
    keys(&mut s, &[Key::Paste("tout\n".into())], &quiet());
    let Overlay::Typing(typing) = &s.overlay else { panic!("not typing") };
    assert_eq!((typing.text.as_str(), typing.cursor), ("«Rôle de tout", 13));
    // A click in it keeps it; elsewhere gives it up.
    let row = s.focusable().iter().position(|f| *f == Fid::Role).unwrap();
    s.click(Target::Row(row), &quiet());
    assert!(matches!(&s.overlay, Overlay::Typing(t) if t.text == "«Rôle de tout"));
    let effects = keys(&mut s, &[Key::Enter], &quiet());
    assert!(
        matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::Set { member: "coordinateur".into(), field: Field::Role("«Rôle de tout".into()) }])
    );
    keys(&mut s, &[Key::Enter], &quiet());
    s.click(Target::Row(0), &quiet());
    assert!(matches!(s.overlay, Overlay::None));
}

#[test]
fn the_team_keys_from_the_list_only() {
    let mut s = sheet();
    to_row(&mut s, Fid::Name);
    assert!(keys(&mut s, &[Key::Char('t')], &quiet()).is_empty(), "a letter in the sheet does nothing");
    keys(&mut s, &[Key::Esc], &quiet());
    let effects = keys(&mut s, &[Key::Char('t')], &quiet());
    assert!(matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::Dashboard(false)]));
    keys(&mut s, &[Key::Char(Action::Stop.key())], &quiet());
    assert!(confirm(&s).danger, "stopping is asked");
    assert_eq!(keys(&mut s, &[Key::Left, Key::Enter], &quiet()), [Effect::Stop]);
    let mut s = sheet();
    s.team.detach = false;
    assert!(keys(&mut s, &[Key::Char('d')], &quiet()).is_empty(), "no client to detach");
}

#[test]
fn a_new_agent() {
    let mut s = sheet();
    keys(&mut s, &[Key::Char('n')], &quiet());
    assert_eq!((s.entry.clone(), s.pane), (Entry::New, Pane::Sheet));
    let roles = s.roles.clone().unwrap();
    assert!(!roles.is_empty());
    assert_eq!(s.draft.name, roles[0].0, "the first role's name");
    to_row(&mut s, Fid::Builtin);
    keys(&mut s, &[Key::Right], &quiet());
    assert_eq!((s.draft.name.as_str(), s.draft.role.as_str()), (roles[1].0.as_str(), roles[1].1.role.as_str()));
    to_row(&mut s, Fid::Add);
    let effects = keys(&mut s, &[Key::Enter], &quiet());
    let [Effect::Apply { edits, select, .. }] = effects.as_slice() else { panic!("{effects:?}") };
    let [Edit::Add { name, member }] = edits.as_slice() else { panic!() };
    assert_eq!((name.as_str(), member.contact), (roles[1].0.as_str(), false));
    assert_eq!(select.as_deref(), Some(name.as_str()));
    // Typed by hand: nothing left of the built-in role; its own name and role, the role required.
    to_row(&mut s, Fid::How);
    keys(&mut s, &[Key::Right, Key::Right], &quiet());
    assert_eq!(s.draft.how, How::Typed);
    assert_eq!((s.draft.name.as_str(), s.draft.role.as_str(), s.draft.base.clone()), ("", "", Member::default()));
    s.draft.name = "testeur".into();
    to_row(&mut s, Fid::Add);
    assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty());
    assert!(matches!(s.said, Some(Said::Failed(_))), "no role");
    to_row(&mut s, Fid::NewRole);
    keys(&mut s, &[Key::Enter], &quiet());
    let Overlay::Typing(typing) = &s.overlay else { panic!("not typing") };
    assert!(typing.error.is_none(), "an empty text is not wrong before it is typed");
}

#[test]
fn claude_composes_one() {
    let mut s = sheet();
    keys(&mut s, &[Key::Char('n')], &quiet());
    to_row(&mut s, Fid::How);
    keys(&mut s, &[Key::Right], &quiet());
    to_row(&mut s, Fid::Compose);
    keys(&mut s, &[Key::Enter], &quiet());
    assert!(matches!(s.overlay, Overlay::Typing(Typing { field: Fid::Request, .. })), "the request first");
    typed(&mut s, "doc");
    assert_eq!(keys(&mut s, &[Key::Enter], &quiet()), [Effect::Compose { id: 1, request: "doc".into() }]);
    to_row(&mut s, Fid::Add);
    assert!(keys(&mut s, &[Key::Enter], &quiet()).is_empty(), "nothing to add before it is composed");
    to_row(&mut s, Fid::Compose);
    assert_eq!(keys(&mut s, &[Key::Esc], &quiet()), [Effect::CancelCompose(1)], "Esc stops it");
    assert_eq!(keys(&mut s, &[Key::Enter], &quiet()), [Effect::Compose { id: 2, request: "doc".into() }]);
    let member = Member { role: "Doc".into(), instructions: Some("- tout".into()), ..Default::default() };
    s.composed(1, Ok(("ancien".into(), member.clone())));
    assert!(!s.draft.composed, "the first request's result, ignored");
    s.composed(2, Ok(("redacteur".into(), member)));
    assert!(s.draft.composed);
    assert_eq!((s.draft.name.as_str(), s.field()), ("redacteur", Some(Fid::Add)));
    to_row(&mut s, Fid::Compose);
    keys(&mut s, &[Key::Enter], &quiet());
    s.composed(3, Err("pas de réseau".into()));
    assert_eq!(s.said, Some(Said::Failed("pas de réseau".into())));
}

#[test]
fn changing_how_stops_claude() {
    let mut s = sheet();
    keys(&mut s, &[Key::Char('n')], &quiet());
    to_row(&mut s, Fid::How);
    keys(&mut s, &[Key::Right], &quiet());
    s.draft.request = "doc".into();
    to_row(&mut s, Fid::Compose);
    keys(&mut s, &[Key::Enter], &quiet());
    to_row(&mut s, Fid::How);
    assert_eq!(keys(&mut s, &[Key::Right], &quiet()), [Effect::CancelCompose(1)]);
    assert_eq!(s.draft.composing, None);
    s.composed(1, Ok(("tard".into(), Member::default())));
    assert!(!s.draft.composed && s.draft.name.is_empty(), "its result comes too late");
}

#[test]
fn lists_of_values() {
    let mut s = sheet();
    to_row(&mut s, Fid::Model);
    keys(&mut s, &[Key::Enter], &quiet());
    let Overlay::List(list) = &s.overlay else { panic!("no list") };
    assert_eq!(list.current, Some(0));
    assert!(list.items[0].label.contains("Opus"), "the default first, and where it comes from");
    assert!(list.items.last().unwrap().other);
    keys(&mut s, &[Key::End, Key::Enter], &quiet());
    assert!(matches!(s.overlay, Overlay::Typing(Typing { field: Fid::Model, .. })), "another one, typed");
    typed(&mut s, "gpt");
    keys(&mut s, &[Key::Enter], &quiet());
    assert!(matches!(s.overlay, Overlay::Confirm(_)), "an unknown model is asked");
    to_row(&mut s, Fid::Mode);
    s.overlay = Overlay::None;
    keys(&mut s, &[Key::Enter], &quiet());
    let Overlay::List(list) = &s.overlay else { panic!("no list") };
    assert!(list.items.iter().skip(1).all(|i| i.detail.is_some()), "each mode said");
    keys(&mut s, &[Key::Esc], &quiet());
    assert!(matches!(s.overlay, Overlay::None));
}

#[test]
fn values_and_where_they_come_from() {
    let mut s = sheet();
    let layers = s.team.people[0].layers.as_mut().unwrap();
    layers.model = layered(Some("haiku"), Source::Personal, Some((Some("sonnet"), Source::Shared)));
    layers.instructions = layered(Some("- un\n\n- deux"), Source::Shared, None);
    assert_eq!(s.value(Fid::Model), "Haiku");
    assert_eq!(s.origin(Fid::Model).as_deref(), Some("settings.local.toml"));
    assert_eq!(
        s.choices(Fid::Model, false)[0].label,
        t!("défaut : Sonnet (settings.toml)", "default: Sonnet (settings.toml)")
    );
    assert_eq!(s.current(Fid::Model), s.choices(Fid::Model, false).iter().position(|c| c.label == "Haiku"));
    assert_eq!(s.value(Fid::Instructions), i18n::count(2, "ligne", "line"));
    assert_eq!(s.origin(Fid::Tab).as_deref(), Some(t!("défaut de recruit", "recruit's default").as_str()));
    assert_eq!(s.choices(Fid::Tab, false)[0].label, t!("défaut : onglets « Agents »", "default: \"Agents\" tabs"));
    assert_eq!(model_label("claude-opus-4-1"), "claude-opus-4-1");
    let global = Origin::Global { file: "/c/recruit/web.toml".into() };
    assert_eq!(source_name(Source::Shared, &global), "web.toml");
}

/// No layer gives the effort: the sheet shows what Claude Code applies, with « Claude Code » as its origin, as the list's
/// default line says it; when that is not known, as before (« défaut de Claude Code »). A layer's value keeps its
/// file.
#[test]
fn what_claude_code_applies_is_shown_with_its_origin() {
    let mut s = sheet();
    // The fixture's coordinateur runs on Opus at high, nothing in the files for the effort.
    assert_eq!(s.value(Fid::Effort), look::effort_label("high"));
    assert_eq!(s.origin(Fid::Effort).as_deref(), Some("Claude Code"));
    assert_eq!(s.effort_level().as_deref(), Some("high"));
    // The model a layer gives is a layer's: its file.
    assert_eq!(s.value(Fid::Model), "Opus");
    assert_eq!(s.origin(Fid::Model).as_deref(), Some(t!("équipe", "team").as_str()));
    // No model in the files either: Opus all the same, from Claude Code.
    s.team.people[0].layers.as_mut().unwrap().model = layered(None, Source::Default, Some((None, Source::Default)));
    assert_eq!((s.value(Fid::Model).as_str(), s.origin(Fid::Model).as_deref()), ("Opus", Some("Claude Code")));
    // Not known yet: as before.
    s.team.people[0].effort = None;
    s.team.people[0].model = None;
    assert_eq!(s.value(Fid::Effort), t!("défaut de Claude Code", "Claude Code's default"));
    assert_eq!(s.value(Fid::Model), t!("défaut de Claude Code", "Claude Code's default"));
    assert_eq!(s.origin(Fid::Effort).as_deref(), Some(t!("défaut", "default").as_str()));
    assert_eq!(s.effort_level(), None);
    // A level a layer gives: its own color and its file.
    s.team.people[0].layers.as_mut().unwrap().effort = layered(Some("xhigh"), Source::Shared, None);
    assert_eq!(s.effort_level().as_deref(), Some("xhigh"));
    assert_eq!(s.origin(Fid::Effort).as_deref(), Some(config::SETTINGS));
}

#[test]
fn the_grid_is_chosen_from_a_list() {
    let mut s = Sheet::new(team(&[("dev", false)]));
    keys(&mut s, &[Key::Char(Action::Grid.key())], &quiet());
    let Overlay::List(list) = &s.overlay else { panic!("no list") };
    let labels: Vec<&str> = list.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["4 × 2", "3 × 2", "2 × 2", "3 × 1", "2 × 1"]);
    assert_eq!(list.current, Some(1), "the grid in place is marked");
    assert_eq!(list.items[0].detail, Some(t!("{} par onglet", "{} per tab", 8)), "how many it holds, beside it");
    keys(&mut s, &[Key::Down], &quiet());
    let effects = s.key(Key::Enter, &quiet());
    assert!(
        matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::Grid { columns: 2, rows: 2 }]),
        "{effects:?}"
    );
    assert!(matches!(s.overlay, Overlay::None) && s.said.is_none());
    // Escape gives up, and says nothing.
    keys(&mut s, &[Key::Char(Action::Grid.key())], &quiet());
    assert!(s.said.is_none());
    assert!(s.key(Key::Esc, &quiet()).is_empty() && s.said.is_none() && matches!(s.overlay, Overlay::None));
}

#[test]
fn a_grid_from_the_files_is_offered_too() {
    let mut s = Sheet::new(team(&[("dev", false)]));
    let layers = s.team.layers.as_mut().unwrap();
    (layers.columns.value, layers.rows.value) = (4, 3);
    keys(&mut s, &[Key::Char(Action::Grid.key())], &quiet());
    let Overlay::List(list) = &s.overlay else { panic!("no list") };
    assert_eq!((list.items[0].label.as_str(), list.current), ("4 × 3", Some(0)));
    assert_eq!(list.items.len(), 6);
    // The default is told apart, whatever the current one.
    let default = t!("défaut", "default");
    assert_eq!(list.items.iter().filter(|i| i.detail.as_deref().is_some_and(|d| d.contains(&default))).count(), 1);
}

#[test]
fn the_default_is_always_offered() {
    let mut s = Sheet::new(team(&[("dev", false)]));
    let layers = s.team.layers.as_mut().unwrap();
    // The shared file says 4 columns, the local one 2 rows: the default under the local layer is 4 × 2.
    layers.columns.fallback = Some((4, Source::Shared));
    layers.rows.fallback = Some((2, Source::Shared));
    layers.auto_compact.value = Some(85);
    layers.auto_compact.fallback = Some((Some(75), Source::Shared));
    keys(&mut s, &[Key::Char(Action::Grid.key())], &quiet());
    let Overlay::List(list) = &s.overlay else { panic!("no list") };
    let word = t!("défaut", "default");
    let default = list.items.iter().find(|i| i.detail.as_deref().is_some_and(|d| d.contains(&word)));
    assert_eq!(default.expect("the default is there").label, "4 × 2");
    s.overlay = Overlay::None;
    keys(&mut s, &[Key::Char(Action::AutoCompact.key())], &quiet());
    let Overlay::List(list) = &s.overlay else { panic!("no list") };
    let default = list.items.iter().find(|i| i.detail.is_some()).expect("the default is there");
    assert_eq!(default.label, t!("{} %", "{}%", 75));
    assert!(list.items.iter().any(|i| i.label == t!("{} %", "{}%", 85)), "and the one in place");
    let places: Vec<u8> = list.items.iter().filter_map(|i| i.value.as_deref()?.parse().ok()).collect();
    assert!(places.is_sorted(), "in order: {places:?}");
}

#[test]
fn the_compaction_share_is_chosen_from_a_list() {
    let mut s = Sheet::new(team(&[("dev", false)]));
    keys(&mut s, &[Key::Char(Action::AutoCompact.key())], &quiet());
    let Overlay::List(list) = &s.overlay else { panic!("no list") };
    let labels: Vec<String> = list.items.iter().map(|i| i.label.clone()).collect();
    let at = |n: u8| t!("{} %", "{}%", n);
    assert_eq!(labels, [t!("non", "off"), at(50), at(60), at(70), at(80), at(90)]);
    assert_eq!(list.current, Some(3), "70 % in place");
    assert_eq!(list.items[3].detail, Some(t!("défaut", "default")));
    keys(&mut s, &[Key::Down], &quiet());
    let effects = s.key(Key::Enter, &quiet());
    assert!(
        matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::AutoCompact(Some(80))]),
        "{effects:?}"
    );
    // Off is the first: None.
    keys(&mut s, &[Key::Char(Action::AutoCompact.key()), Key::Home], &quiet());
    let effects = s.key(Key::Enter, &quiet());
    assert!(matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::AutoCompact(None)]));
}

#[test]
fn a_share_from_the_files_finds_its_place() {
    let mut s = Sheet::new(team(&[("dev", false)]));
    s.team.layers.as_mut().unwrap().auto_compact.value = Some(75);
    keys(&mut s, &[Key::Char(Action::AutoCompact.key())], &quiet());
    let Overlay::List(list) = &s.overlay else { panic!("no list") };
    let labels: Vec<&str> = list.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels[4], t!("{} %", "{}%", 75));
    assert_eq!(list.current, Some(4));
    s.team.layers.as_mut().unwrap().auto_compact.value = None;
    s.overlay = Overlay::None;
    keys(&mut s, &[Key::Char(Action::AutoCompact.key())], &quiet());
    let Overlay::List(list) = &s.overlay else { panic!("no list") };
    assert_eq!(list.current, Some(0), "off in place");
}

#[test]
fn no_grid_with_a_tab_per_member() {
    let mut s = Sheet::new(team(&[("dev", false)]));
    assert!(s.actions().contains(&Action::Grid));
    s.team.layers.as_mut().unwrap().layout.value = config::Layout::Tabs;
    assert!(!s.actions().contains(&Action::Grid) && s.actions().contains(&Action::AutoCompact));
    assert!(!s.actions().contains(&Action::Layout), "nor a focus");
    keys(&mut s, &[Key::Char(Action::Grid.key())], &quiet());
    assert!(matches!(s.overlay, Overlay::None));
}

#[test]
fn the_layout_goes_from_grid_to_focus_and_back() {
    let mut s = Sheet::new(team(&[("dev", false)]));
    let effects = s.key(Key::Char(Action::Layout.key()), &quiet());
    assert!(
        matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::Layout(config::Layout::Focus)]),
        "{effects:?}"
    );
    s.team.layers.as_mut().unwrap().layout.value = config::Layout::Focus;
    let effects = s.key(Key::Char(Action::Layout.key()), &quiet());
    assert!(matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[Edit::Layout(config::Layout::Auto)]));
}

#[test]
fn the_settings_wait_for_readable_files() {
    let mut s = Sheet::new(team(&[("dev", false)]));
    s.team.unreadable = Some("boom".into());
    s.team.layers = None;
    assert_eq!(s.actions(), [Action::Detach, Action::Stop]);
    s.team.unreadable = None;
    assert!(!s.actions().contains(&Action::Grid), "no layers, no setting to show");
}

#[test]
fn hints() {
    let mut s = sheet();
    s.choose(Entry::Member("dev".into()));
    to_row(&mut s, Fid::Model);
    assert!(s.hint().unwrap().ends_with(&t!("effet dès sa prochaine requête", "in effect from its next request")));
    s.team.people[1].live = false;
    assert!(
        s.hint().unwrap().ends_with(&t!("il sera relancé sur sa conversation", "it restarts on its conversation")),
        "without the mod"
    );
    to_row(&mut s, Fid::Tab);
    assert!(s.hint().unwrap().starts_with(&t!("défaut : onglets", "default: \"Agents\"")), "{:?}", s.hint());
    to_row(&mut s, Fid::Contact);
    assert!(s.hint().unwrap().starts_with(&t!("défaut : non", "default: no")));
}

#[test]
fn clicks() {
    let mut s = sheet();
    s.click(Target::Entry(Entry::Member("ops".into())), &quiet());
    assert_eq!((s.entry.clone(), s.pane), (Entry::Member("ops".into()), Pane::List));
    s.click(Target::Row(1), &quiet());
    assert_eq!(s.field(), Some(Fid::Effort));
    s.click(Target::Row(1), &quiet());
    assert!(matches!(s.overlay, Overlay::List(_)), "a second click opens it");
    let effects = s.click(Target::Item(0), &quiet());
    assert!(effects.len() <= 1);
    let effects = s.click(Target::Step(1, true), &quiet());
    let low = Edit::Set { member: "ops".into(), field: Field::Effort(Some("low".into())) };
    assert!(
        matches!(&effects[..], [Effect::Apply { edits, .. }] if edits == &[low]),
        "carried by its requests: at once"
    );
    s.click(Target::Action(Action::RestartAll), &quiet());
    assert!(s.click(Target::Row(0), &quiet()).is_empty(), "a dialog stays until answered");
    assert_eq!(s.click(Target::Yes, &quiet()), [Effect::RestartAll]);
    s.click(Target::Restart(true), &quiet());
    assert!(matches!(s.overlay, Overlay::Confirm(_)), "afresh is asked");
    s.click(Target::No, &quiet());
    assert!(matches!(s.overlay, Overlay::None));
}

#[test]
fn a_member_gone_from_the_files() {
    let mut s = sheet();
    s.team.people[2].gone = true;
    s.team.people[2].layers = None;
    s.choose(Entry::Member("ops".into()));
    assert_eq!(s.focusable(), [Fid::Close]);
    to_row(&mut s, Fid::Close);
    keys(&mut s, &[Key::Enter], &quiet());
    assert_eq!(keys(&mut s, &[Key::Left, Key::Enter], &quiet()), [Effect::Dismiss("ops".into())]);
}

#[test]
fn files_that_do_not_read() {
    let mut t = team(&[("coordinateur", true), ("dev", false)]);
    t.unreadable = Some("settings.toml: ligne 3".into());
    for p in &mut t.people {
        p.layers = None;
    }
    let mut s = seen(t);
    assert!(matches!(&s.said, Some(Said::Failed(said)) if said.contains("ligne 3")), "said at once");
    assert!(!s.entries().contains(&Entry::New));
    assert!(s.focusable().is_empty(), "nothing to change");
    assert_eq!(s.actions(), [Action::Detach, Action::Stop]);
    keys(&mut s, &[Key::Enter, Key::Char('t'), Key::Char('n')], &quiet());
    assert_eq!(s.pane, Pane::List);
    assert!(matches!(s.overlay, Overlay::None), "nothing opened");
    keys(&mut s, &[Key::Char(Action::Stop.key())], &quiet());
    assert_eq!(keys(&mut s, &[Key::Left, Key::Enter], &quiet()), [Effect::Stop]);
}

#[test]
fn instructions_edited() {
    let mut s = sheet();
    assert!(s.edited(Edited::Instructions("dev".into()), Err("vi : introuvable".into())).is_empty());
    assert!(matches!(&s.said, Some(Said::Failed(said)) if said.contains("introuvable")));
    assert!(s.edited(Edited::Instructions("dev".into()), Ok("\n".into())).is_empty(), "none before, none after");
    let effects = s.edited(Edited::Instructions("dev".into()), Ok("- tout\n".into()));
    let [Effect::Apply { edits, .. }] = effects.as_slice() else { panic!("{effects:?}") };
    assert_eq!(edits, &[Edit::Set { member: "dev".into(), field: Field::Instructions(Some("- tout".into())) }]);
    s.edited(Edited::Draft, Ok("  - brouillon \n".into()));
    assert_eq!(s.draft.instructions.as_deref(), Some("- brouillon"));
    assert_eq!(s.to_edit(&Edited::Draft), "- brouillon");
}

#[test]
fn reloaded_after_a_change() {
    let mut s = sheet();
    s.choose(Entry::Member("ops".into()));
    s.reload(team(&[("coordinateur", true), ("dev", false)]), None);
    assert_eq!(
        (s.entry.clone(), s.pane),
        (Entry::Member("coordinateur".into()), Pane::List),
        "the first one, ops gone"
    );
    s.reload(team(&[("coordinateur", true), ("release", false)]), Some("release".into()));
    assert_eq!(s.entry, Entry::Member("release".into()));
}

#[test]
fn short_texts() {
    assert_eq!(first_line("  Écrit le code\nen détail", 50), "Écrit le code");
    assert_eq!(first_line("abcdef ghij", 8), "abcdef…");
    // Wide characters count two columns each.
    assert_eq!(first_line("日本語日本語", 7), "日本語…");
    assert_eq!(first_line("日本語", 6), "日本語");
    if i18n::lang() == i18n::Lang::Fr {
        assert_eq!(quoted(&["a", "b", "c"]), "« a », « b » et « c »");
        assert_eq!(i18n::joined(i18n::lang(), &["a", "b"], "ou"), "« a » ou « b »");
        assert_eq!(instructions_count(None), "aucune");
    }
}

//! What happens as the text changes: protected spans, phrase hints, changes against the original,
//! spelling and grammar, the word under the cursor and its suggestions, sentence rewrites,
//! auto-rewrite and its review mode, and the editor's context menu.

use crate::app::{App, AppRc, Proposal, Snap, Target};
use crate::engine::{self, AutoOptions, Issue};
use crate::theme::MOD;
use crate::widgets;
use crate::workers::{self, Cancel};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Sentences the context model reads per pass, nearest to the view first.
const GRAMMAR_BATCH: usize = 300;

thread_local! {
    static APP: RefCell<Weak<App>> = RefCell::new(Weak::new());
}

/// Run `f` with the app, on the UI thread (for results posted from worker threads).
pub fn with_app(f: impl FnOnce(&AppRc)) {
    if let Some(app) = APP.with(|a| a.borrow().upgrade()) {
        f(&app);
    }
}

static PROOF_GEN: AtomicU64 = AtomicU64::new(0);
type GramKey = (String, Vec<(usize, usize)>);
static GRAM_CACHE: Mutex<Option<(HashMap<GramKey, Vec<engine::Slip>>, Vec<GramKey>)>> = Mutex::new(None);

pub fn clear_grammar_cache() {
    *GRAM_CACHE.lock().unwrap() = None;
}

fn later(app: &App, name: &str, ms: u32, f: fn(&AppRc)) {
    let me = app.weak();
    workers::later(name, ms, move || {
        if let Some(a) = me.upgrade() {
            f(&a)
        }
    });
}

impl App {
    /// Hook the editor's signals: edits, the cursor, clicks and the context menu.
    pub fn connect_editor(self: &Rc<Self>) {
        APP.with(|a| *a.borrow_mut() = Rc::downgrade(self));
        let me = self.weak();
        self.buf.connect_changed(move |_| {
            if let Some(app) = me.upgrade() {
                app.on_modified();
            }
        });
        let me = self.weak();
        self.buf.connect_mark_set(move |_, _, mark| {
            let Some(app) = me.upgrade() else { return };
            let name = mark.name();
            if name.as_deref() == Some("insert") || name.as_deref() == Some("selection_bound") {
                later(&app, "target", 280, |a| a.update_target(false));
                if app.find().visible() {
                    app.find().show_count();
                }
            }
        });
        let keys = gtk::EventControllerKey::new();
        let me = self.weak();
        keys.connect_key_released(move |_, key, _, _| {
            use gdk::Key;
            if matches!(key, Key::Shift_L | Key::Shift_R | Key::Control_L | Key::Control_R | Key::Alt_L | Key::Alt_R | Key::Meta_L | Key::Meta_R) {
                return;
            }
            if let Some(app) = me.upgrade() {
                later(&app, "target", 350, |a| a.update_target(false));
            }
        });
        self.text.add_controller(keys);
        let click = gtk::GestureClick::new();
        click.set_button(1);
        let me = self.weak();
        click.connect_released(move |_, _, _, _| {
            if let Some(app) = me.upgrade() {
                later(&app, "target", 60, |a| a.update_target(false));
            }
        });
        self.text.add_controller(click);
        let menu = gtk::GestureClick::new();
        menu.set_button(gdk::BUTTON_SECONDARY);
        menu.set_propagation_phase(gtk::PropagationPhase::Capture);
        let me = self.weak();
        menu.connect_pressed(move |g, _, x, y| {
            g.set_state(gtk::EventSequenceState::Claimed);
            if let Some(app) = me.upgrade() {
                app.context_menu(x, y);
            }
        });
        self.text.add_controller(menu);
        if cfg!(target_os = "macos") {
            // a Mac's usual right-click is Control+click
            let ctrl = gtk::GestureClick::new();
            ctrl.set_button(gdk::BUTTON_PRIMARY);
            ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
            let me = self.weak();
            ctrl.connect_pressed(move |g, _, x, y| {
                if !g.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                    return;
                }
                g.set_state(gtk::EventSequenceState::Claimed);
                if let Some(app) = me.upgrade() {
                    app.context_menu(x, y);
                }
            });
            self.text.add_controller(ctrl);
        }
        // a paste into an empty, unsaved document becomes its original
        let me = self.weak();
        self.text.connect_paste_clipboard(move |_| {
            let Some(app) = me.upgrade() else { return };
            if app.mode() == "plain" && app.plain_text().trim().is_empty() {
                later(&app, "paste", 10, |app| {
                    if app.st.borrow().doc.path.is_some() {
                        return;
                    }
                    let txt = app.plain_text();
                    {
                        let mut st = app.st.borrow_mut();
                        st.original = txt.clone();
                        if st.doc.kind == "plain" {
                            st.doc.kind = rewrite_core::files::sniff_kind(&txt).to_string();
                        }
                    }
                    app.update_title();
                    app.analyze();
                    if app.st.borrow().preview_on {
                        app.preview().refresh(false);
                    }
                });
            }
        });
    }

    fn on_modified(&self) {
        if self.st.borrow().loading {
            return;
        }
        let first = {
            let mut st = self.st.borrow_mut();
            st.snap_dirty = true;
            let first = !st.dirty;
            st.dirty = true;
            first
        };
        if first {
            self.update_title();
        }
        later(self, "analyze", 400, |a| a.analyze());
    }

    pub fn analyze(&self) {
        let snap = self.refresh_snapshot();
        let text = &snap.text;
        let had = !self.st.borrow().proposals.is_empty();
        if had {
            let before = self.st.borrow().proposals.len();
            self.prune_props();
            if self.st.borrow().proposals.len() != before {
                self.update_review();
            }
        }
        let stale = self.st.borrow().sent_target.as_ref().is_some_and(|(s, e, t, _)| snap.get(*s, *e) != t);
        if stale {
            // the sentence was edited: its rewrites are stale
            let mut st = self.st.borrow_mut();
            st.sent_target = None;
            st.sent_items.clear();
            drop(st);
            self.panel().set_sentence("idle", vec![]);
        }
        let protected = if self.rich_active() {
            let mut p = self.rich().protected(&snap);
            p.extend(engine::protected_spans(text, "plain", None));
            engine::merge(p)
        } else {
            let (kind, path) = {
                let st = self.st.borrow();
                (st.doc.kind.clone(), st.doc.path.as_ref().map(|p| p.to_string_lossy().into_owned()))
            };
            engine::protected_spans(text, &kind, path.as_deref())
        };
        self.retag(&self.tags.prot, &protected);
        let hints: Vec<(usize, usize)> =
            engine::phrase_spans(text).into_iter().filter(|&(s, e)| !engine::overlaps(&protected, s, e)).collect();
        self.retag(&self.tags.hint, &hints);
        {
            let mut st = self.st.borrow_mut();
            st.protected = protected;
            st.hint_spans = hints;
        }
        let orig = self.st.borrow().original.clone();
        let cur = text.clone();
        let me = self.weak();
        let snap2 = snap.clone();
        self.lanes.diff.submit(
            move || engine::compute_changes(&orig, &cur),
            move |res| {
                if let (Some(app), Ok(res)) = (me.upgrade(), res) {
                    app.changes_done(&snap2, res);
                }
            },
        );
        self.proofread(&snap, None);
        if self.st.borrow().preview_on {
            self.preview().text_changed();
        }
        if self.find().visible() {
            self.find().search(false);
        }
    }

    /// Put a decoration tag on exactly these spans.
    pub fn retag(&self, tag: &gtk::TextTag, spans: &[(usize, usize)]) {
        let buf = &self.buf;
        buf.remove_tag(tag, &buf.start_iter(), &buf.end_iter());
        for &(s, e) in spans {
            if e > s {
                buf.apply_tag(tag, &self.iter_at(s), &self.iter_at(e));
            }
        }
    }

    fn changes_done(&self, snap: &Rc<Snap>, res: (Vec<(usize, usize, usize, usize)>, f64)) {
        if !Rc::ptr_eq(snap, &self.st.borrow().snap) {
            return; // the text moved on; a newer diff is coming
        }
        let spans: Vec<(usize, usize)> = res.0.iter().map(|&(a, b, _, _)| (a, b)).collect();
        {
            let mut st = self.st.borrow_mut();
            st.changes = res.0;
            st.change_ratio = res.1;
        }
        self.retag(&self.tags.chg, &spans);
        self.update_stats();
        self.update_original_section();
    }

    pub fn on_scroll(&self) {
        if self.find().visible() {
            later(self, "find-view", 80, |a| a.find().paint());
        }
        if self.st.borrow().proof_more {
            // a long text is checked around the view; follow it
            later(self, "proof-view", 600, |a| {
                let base = a.st.borrow().proof_base.clone();
                if let Some((snap, issues)) = base {
                    let cur = Rc::ptr_eq(&snap, &a.st.borrow().snap) && !a.st.borrow().snap_dirty && !a.busy();
                    if cur {
                        a.proofread(&snap, Some(issues));
                    }
                }
            });
        }
    }

    /// The character offset at the top of the view.
    fn view_top(&self) -> usize {
        let rect = self.text.visible_rect();
        self.text.iter_at_location(rect.x(), rect.y()).map_or(0, |i| i.offset() as usize)
    }

    /// Check the snapshot in the background; underlines appear as the results come in. `base` = the
    /// spelling and rule remarks of this snapshot, when they are already known.
    fn proofread(&self, snap: &Rc<Snap>, base: Option<Vec<Issue>>) {
        let generation = {
            let mut st = self.st.borrow_mut();
            st.proof_gen += 1;
            st.proof_gen
        };
        PROOF_GEN.store(generation, Ordering::SeqCst);
        let (spell, gram) = (self.settings().spell_check, self.settings().grammar_check);
        if !(spell || gram) || snap.text.trim().is_empty() {
            self.proof_done(generation, snap, vec![], vec![], false);
            return;
        }
        let text = snap.text.clone();
        let prot = self.st.borrow().protected.clone();
        let eng = self.engine.clone();
        let lines = self.rich_active();
        let focus = self.view_top();
        let interactive = [self.lanes.word.activity(), self.lanes.sent.activity(), self.lanes.slow.activity()];
        let snap_ui = glib::thread_guard::ThreadGuard::new(snap.clone());
        let snap_ui = Arc::new(Mutex::new(Some(snap_ui)));
        let post = move |first: Vec<Issue>, slips: Vec<Issue>, more: bool, last: bool| {
            let s = snap_ui.clone();
            workers::post(move || {
                let guard = if last { s.lock().unwrap().take() } else { None };
                let snap = match guard {
                    Some(g) => Some(g.into_inner()),
                    None => s.lock().unwrap().as_ref().map(|g| g.get_ref().clone()),
                };
                if let Some(snap) = snap {
                    with_app(|app| app.proof_done(generation, &snap, first, slips, more));
                }
            });
        };
        self.lanes.proof.run(move || {
            let first = match base {
                Some(b) => b,
                None => {
                    let mut first = vec![];
                    if spell {
                        first.extend(eng.spell_check(&text, &prot).into_iter().map(|(s, e)| Issue {
                            start: s,
                            end: e,
                            kind: "spell".into(),
                            message: String::new(),
                            fixes: vec![],
                        }));
                    }
                    if gram {
                        first.extend(eng.grammar_rules(&text, &prot));
                    }
                    post(first.clone(), vec![], false, false);
                    first
                }
            };
            if !(gram && eng.has_context_model()) {
                post(first, vec![], false, true);
                return;
            }
            // The context model reads one sentence at a time, starting with what is on screen. A long text
            // is not read to the end: the rest follows when the view moves there.
            let off = engine::Offsets::new(&text);
            let mut sents = engine::sentence_bounds(&text, lines);
            sents.sort_by_key(|&(s, _)| (s as i64 - focus as i64).abs());
            let (mut slips, mut budget, mut more) = (vec![], GRAMMAR_BATCH, false);
            let (mut last, mut fresh) = (Instant::now(), false);
            for (s, e) in sents {
                let sent = engine::slice(&text, &off, s, e);
                let spans = engine::clip(&prot, s, e);
                let key = (sent.clone(), spans.clone());
                let cached = GRAM_CACHE.lock().unwrap().as_ref().and_then(|(m, _)| m.get(&key).cloned());
                let found = match cached {
                    Some(f) => f,
                    None => {
                        if budget == 0 {
                            more = true;
                            continue;
                        }
                        budget -= 1;
                        while interactive.iter().any(|l| l.active()) {
                            // suggestions the user asked for come first
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        if PROOF_GEN.load(Ordering::SeqCst) != generation {
                            return; // the text changed; a newer check has started
                        }
                        let f = eng.grammar(&sent, &spans);
                        let mut g = GRAM_CACHE.lock().unwrap();
                        let (m, order) = g.get_or_insert_with(Default::default);
                        m.insert(key.clone(), f.clone());
                        order.push(key);
                        if order.len() > 8000 {
                            let old = order.remove(0);
                            m.remove(&old);
                        }
                        f
                    }
                };
                for (a, b, msg, fixes) in found {
                    slips.push(Issue { start: s + a, end: s + b, kind: "gram".into(), message: msg, fixes });
                    fresh = true;
                }
                if fresh && last.elapsed().as_secs_f64() > 0.5 {
                    last = Instant::now();
                    fresh = false;
                    post(first.clone(), slips.clone(), true, false);
                }
            }
            post(first, slips, more, true);
        });
    }

    fn proof_done(&self, generation: u64, snap: &Rc<Snap>, base: Vec<Issue>, slips: Vec<Issue>, more: bool) {
        {
            let st = self.st.borrow();
            if generation != st.proof_gen || !Rc::ptr_eq(snap, &st.snap) {
                return;
            }
        }
        {
            let mut st = self.st.borrow_mut();
            st.proof_base = Some((snap.clone(), base.clone()));
            st.proof_more = more;
            st.found = base.into_iter().chain(slips).collect();
        }
        self.show_issues();
    }

    fn show_issues(&self) {
        let Some((snap, _)) = self.st.borrow().proof_base.clone() else { return };
        let issues: Vec<Issue> = {
            let st = self.st.borrow();
            engine::dedupe(st.found.clone())
                .into_iter()
                .filter(|it| !st.ignored.iter().any(|(t, m)| t == snap.get(it.start, it.end) && m == &it.message))
                .collect()
        };
        let spell: Vec<(usize, usize)> = issues.iter().filter(|i| i.kind == "spell").map(|i| (i.start, i.end)).collect();
        let gram: Vec<(usize, usize)> = issues.iter().filter(|i| i.kind == "gram").map(|i| (i.start, i.end)).collect();
        {
            let mut st = self.st.borrow_mut();
            st.issues = issues;
            st.issues_snap = Some(snap);
        }
        self.retag(&self.tags.spell, &spell);
        self.retag(&self.tags.gram, &gram);
        self.update_issue();
        let t = self.st.borrow().target.clone();
        if let Some(t) = t {
            if (t.kind == "word" || t.kind == "spell") && (t.kind == "spell") != self.issue_at(t.start, t.end, Some("spell")).is_some() {
                later(self, "target", 10, |a| a.update_target(true)); // the word's verdict changed
            }
        }
    }

    /// The proofreading remark that touches [s, e], if the remarks still match the text.
    pub fn issue_at(&self, s: usize, e: usize, kind: Option<&str>) -> Option<Issue> {
        let st = self.st.borrow();
        let current = st.issues_snap.as_ref().is_some_and(|sn| Rc::ptr_eq(sn, &st.snap)) && !st.snap_dirty;
        if !current {
            return None;
        }
        st.issues.iter().find(|it| it.start <= e && s <= it.end && kind.is_none_or(|k| it.kind == k)).cloned()
    }

    /// Show the grammar remark under the cursor (or selection) in the panel.
    pub fn update_issue(&self) {
        let mut it = None;
        if !self.busy() {
            let (a, b) = self.selection().unwrap_or_else(|| {
                let c = self.cursor_offset();
                (c, c)
            });
            it = self.issue_at(a, b, Some("gram"));
        }
        let snap = self.st.borrow().snap.clone();
        self.st.borrow_mut().current_issue = it.clone();
        self.panel().set_issue(it.map(|it| (snap.get(it.start, it.end).to_string(), it.message.clone(), it.fixes.clone())));
    }

    pub fn fix_issue(&self, n: usize) {
        let Some(it) = self.st.borrow().current_issue.clone() else { return };
        if self.busy() || n >= it.fixes.len() || self.issue_at(it.start, it.end, Some("gram")).as_ref() != Some(&it) {
            return;
        }
        let old = self.st.borrow().snap.get(it.start, it.end).to_string();
        if self.replace(it.start, it.end, &old, &it.fixes[n]) {
            later(self, "target", 30, |a| a.update_target(true));
        }
    }

    pub fn ignore_issue(&self) {
        let Some(it) = self.st.borrow().current_issue.clone() else { return };
        if self.issue_at(it.start, it.end, Some("gram")).as_ref() == Some(&it) {
            let text = self.st.borrow().snap.get(it.start, it.end).to_string();
            self.st.borrow_mut().ignored.push((text, it.message.clone()));
            self.show_issues();
        }
    }

    /// Accept the misspelled word under the cursor: for good, or (Ignore) until the app closes.
    pub fn add_word(&self, session: bool) {
        let Some(t) = self.st.borrow().target.clone() else { return };
        if t.kind != "spell" {
            return;
        }
        self.engine.add_word(&t.text, session);
        if !session {
            {
                let mut s = self.settings_mut();
                let w = t.text.to_lowercase();
                if !s.user_words.contains(&w) {
                    s.user_words.push(w);
                }
            }
            self.save_settings();
            self.flash_msg(&format!("Added “{}” to your dictionary.", t.text), 5000);
        }
        self.analyze();
        later(self, "target", 30, |a| a.update_target(true));
    }

    pub fn update_stats(&self) {
        if self.mode() == "pdf" {
            return; // the status bar shows the page count (pdf_changed)
        }
        let st = self.st.borrow();
        let n = engine::word_count(&st.snap.text);
        let mut parts = vec![format!("{} words", thousands(n))];
        if !st.original.is_empty() && st.original != st.snap.text {
            parts.push(format!("{:.0}% reworded vs. original", st.change_ratio * 100.0));
        }
        self.w().status_stats.set_label(&parts.join("   ·   "));
    }

    fn change_at(&self, s: usize, e: usize) -> Option<(usize, usize, usize, usize)> {
        self.st.borrow().changes.iter().find(|c| c.0 <= e && s <= c.1).copied()
    }

    pub fn update_original_section(&self) {
        if self.st.borrow().original.is_empty() || self.busy() {
            self.panel().set_original(None);
            return;
        }
        let t = self.st.borrow().target.clone();
        let ch = match t {
            Some(t) => self.change_at(t.start, t.end),
            None => {
                let c = self.cursor_offset();
                self.change_at(c, c)
            }
        };
        self.st.borrow_mut().current_change = ch;
        let orig = self.st.borrow().original.clone();
        let off = engine::Offsets::new(&orig);
        self.panel().set_original(ch.map(|c| engine::slice(&orig, &off, c.2, c.3)));
    }

    /// The sentence(s) around [s, e) plus the previous sentence, as (ctx_start, ctx_end).
    fn context(&self, snap: &Snap, s: usize, e: usize) -> (usize, usize) {
        let text = &snap.text;
        let n = snap.len();
        let ws = s.saturating_sub(3000);
        let sb = snap.off.byte(ws);
        let para = text[sb..snap.off.byte(s)].rfind("\n\n").map(|p| snap.off.char(sb + p));
        let ws = para.map_or(ws, |p| p + 2);
        let eb = snap.off.byte(e);
        let we = text[eb..snap.off.byte((e + 3000).min(n))].find("\n\n").map_or((e + 3000).min(n), |p| snap.off.char(eb + p));
        let sents: Vec<(usize, usize)> =
            engine::sentence_bounds(snap.get(ws, we), self.rich_active()).into_iter().map(|(a, b)| (ws + a, ws + b)).collect();
        let cur: Vec<usize> = sents.iter().enumerate().filter(|(_, (a, b))| *a <= e && s <= *b).map(|(k, _)| k).collect();
        if cur.is_empty() {
            return (ws.max(s.saturating_sub(300)), we.min(e + 300));
        }
        let (first, last) = (cur[0], cur[cur.len() - 1]);
        let cs = if first > 0 && sents[first].0 - sents[first - 1].1 < 3 { sents[first - 1].0 } else { sents[first].0 };
        (cs.min(s), sents[last].1.max(e))
    }

    fn sentence_at(&self, snap: &Snap, off: usize) -> Option<(usize, usize)> {
        let ws = off.saturating_sub(4000);
        let we = (off + 4000).min(snap.len());
        engine::sentence_bounds(snap.get(ws, we), self.rich_active())
            .into_iter()
            .map(|(a, b)| (ws + a, ws + b))
            .find(|&(a, b)| a <= off && off <= b)
    }

    pub fn update_target(&self, force: bool) {
        if self.busy() {
            return;
        }
        self.update_review();
        let snap = self.refresh_snapshot();
        let words = || engine::words(&snap.text);
        let (mut s, mut e): (Option<usize>, Option<usize>) = (None, None);
        let mut kind = "word".to_string();
        let sel = self.selection();
        let is_alnum = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric());
        if let Some((mut a, mut b)) = sel {
            while a < b && !is_alnum(snap.char_at(a)) {
                a += 1;
            }
            while b > a && !(is_alnum(snap.char_at(b - 1)) || snap.char_at(b - 1) == Some('\'')) {
                b -= 1;
            }
            let n = engine::word_count(snap.get(a, b));
            if n == 0 || b - a > 700 {
            } else if n >= 2 {
                (s, e, kind) = (Some(a), Some(b), "phrase".into());
            } else if let Some(&(ws, we)) = words().iter().find(|&&(ws, we)| ws <= a && a < we) {
                (s, e) = (Some(ws), Some(we));
            }
        }
        let mut hint: Option<(usize, usize)> = None;
        if sel.is_none() {
            let off = self.cursor_offset();
            if self.settings().phrase_hints {
                hint = self
                    .st
                    .borrow()
                    .hint_spans
                    .iter()
                    .copied()
                    .find(|&(a, b)| a <= off && off <= b && snap.get(a, b).contains(' ') && engine::phrase_known(snap.get(a, b)));
            }
            if let Some(&(ws, we)) = words().iter().find(|&&(ws, we)| ws <= off && off <= we) {
                (s, e) = (Some(ws), Some(we));
            } else if let Some(h) = hint {
                (s, e, kind, hint) = (Some(h.0), Some(h.1), "phrase".into(), None);
            }
            if hint.is_some() && (s, e) == (hint.map(|h| h.0), hint.map(|h| h.1)) {
                hint = None;
            }
        }
        self.update_issue();
        let (Some(mut s), Some(mut e)) = (s, e) else {
            self.buf.remove_tag(&self.tags.target, &self.buf.start_iter(), &self.buf.end_iter());
            if self.st.borrow().target.is_some() {
                self.st.borrow_mut().target = None;
                self.panel().set_target(None);
            }
            self.update_original_section();
            return;
        };
        if kind == "word" {
            if let Some(typo) = self.issue_at(s, e, Some("spell")) {
                (s, e, kind, hint) = (typo.start, typo.end, "spell".into(), None);
            }
        }
        let text_se = snap.get(s, e).to_string();
        if !force {
            if let Some(t) = &self.st.borrow().target {
                if (t.start, t.end, t.text.as_str()) == (s, e, text_se.as_str()) {
                    return;
                }
            }
        }
        self.buf.remove_tag(&self.tags.target, &self.buf.start_iter(), &self.buf.end_iter());
        self.buf.apply_tag(&self.tags.target, &self.iter_at(s), &self.iter_at(e));
        let id = {
            let mut st = self.st.borrow_mut();
            st.next_target += 1;
            st.next_target
        };
        let target = Target {
            start: s,
            end: e,
            text: text_se.clone(),
            kind: kind.clone(),
            items: None,
            hint,
            hint_text: hint.map(|(a, b)| snap.get(a, b).to_string()),
            id,
        };
        self.st.borrow_mut().target = Some(target.clone());
        self.update_original_section();
        if engine::overlaps(&self.st.borrow().protected, s, e) {
            if let Some(t) = self.st.borrow_mut().target.as_mut() {
                t.items = Some(vec![]);
            }
            self.panel().set_target(Some(crate::panel::TargetView::protected(&text_se, &kind)));
            return;
        }
        let (cs, ce) = self.context(&snap, hint.map_or(s, |h| s.min(h.0)), hint.map_or(e, |h| e.max(h.1)));
        let ctx = snap.get(cs, ce).to_string();
        let (a, b) = (s - cs, e - cs);
        let (ha, hb) = hint.map_or((0, 0), |h| (h.0 - cs, h.1 - cs));
        let spell = kind == "spell";
        let key = format!("{ctx}\u{1}{a}\u{1}{b}\u{1}{ha}\u{1}{hb}\u{1}{spell}\u{1}{}\u{1}{}", self.engine.smart(), self.engine.has_paraphraser());
        let cached = self.st.borrow().cache.get(&key).cloned();
        if let Some(res) = cached {
            self.on_suggestions(id, Ok(res));
            return;
        }
        self.panel().set_target(Some(crate::panel::TargetView::loading(&text_se, &kind)));
        let eng = self.engine.clone();
        let has_hint = hint.is_some();
        let me = self.weak();
        self.lanes.word.submit(
            move || {
                if spell {
                    return (eng.spell_suggest(&ctx, a, b, 8), vec![]);
                }
                let words = eng.suggest(&ctx, a, b, 12, true);
                // the cursor sits inside a highlighted phrase: offer whole-phrase rewrites too
                let phrase = if has_hint { eng.suggest(&ctx, ha, hb, 5, false) } else { vec![] };
                (words, phrase)
            },
            move |res| {
                let Some(app) = me.upgrade() else { return };
                if let Ok(r) = &res {
                    let mut st = app.st.borrow_mut();
                    st.cache.insert(key.clone(), r.clone());
                    st.cache_order.push(key);
                    if st.cache_order.len() > 300 {
                        let old = st.cache_order.remove(0);
                        st.cache.remove(&old);
                    }
                }
                app.on_suggestions(id, res);
            },
        );
    }

    fn on_suggestions(&self, id: u64, res: Result<(Vec<engine::Suggestion>, Vec<engine::Suggestion>), String>) {
        let Some(mut t) = self.st.borrow().target.clone() else { return };
        if t.id != id {
            return;
        }
        let (err, (words, phrase)) = match res {
            Ok(r) => (None, r),
            Err(e) => (Some(e), (vec![], vec![])),
        };
        let mut items: Vec<(engine::Suggestion, usize, usize, String)> = words.iter().map(|x| (x.clone(), t.start, t.end, t.text.clone())).collect();
        let mut groups = vec![(t.kind.clone(), t.text.clone(), words.clone(), 0usize)];
        if let (Some((hs, he)), false) = (t.hint, phrase.is_empty()) {
            let ht = t.hint_text.clone().unwrap_or_default();
            groups.push(("hint".into(), ht.clone(), phrase.clone(), items.len()));
            items.extend(phrase.iter().map(|x| (x.clone(), hs, he, ht.clone())));
        }
        t.items = Some(items);
        self.st.borrow_mut().target = Some(t.clone());
        self.panel().set_target(Some(crate::panel::TargetView::groups(&t.text, &t.kind, groups, err.map(|e| format!("Something went wrong: {e}")))));
        let pending = std::mem::take(&mut self.st.borrow_mut().popup_pending);
        if pending {
            self.popup_at_cursor();
        }
    }

    pub fn apply_suggestion(&self, n: usize) {
        let Some(t) = self.st.borrow().target.clone() else { return };
        let Some(items) = &t.items else { return };
        if self.busy() || n >= items.len() {
            return;
        }
        let (sug, s, e, old) = items[n].clone();
        if !self.replace(s, e, &old, &sug.text) {
            return;
        }
        if t.kind == "phrase" && (s, e) == (t.start, t.end) {
            self.buf.select_range(&self.iter_at(s), &self.iter_at(s + engine::char_len(&sug.text)));
        }
        later(self, "target", 30, |a| a.update_target(false));
    }

    /// Make the edits inside one step for undo.
    pub fn one_step(&self, f: impl FnOnce()) {
        self.buf.begin_user_action();
        f();
        self.buf.end_user_action();
    }

    pub fn replace(&self, s: usize, e: usize, expected: &str, new: &str) -> bool {
        let snap = self.refresh_snapshot();
        if snap.get(s, e) != expected {
            self.bell();
            self.flash_msg("The text changed — click the word again.", 5000);
            return false;
        }
        self.one_step(|| self.put_text(&snap, s, e, new));
        self.st.borrow_mut().snap_dirty = true;
        let end = s + engine::char_len(new);
        self.buf.place_cursor(&self.iter_at(end));
        self.flash(s, end, 700);
        self.text.scroll_mark_onscreen(&self.buf.get_insert());
        self.analyze();
        true
    }

    /// Show the flash highlight on [s, e) for a moment.
    pub fn flash(&self, s: usize, e: usize, ms: u32) {
        self.buf.remove_tag(&self.tags.flash, &self.buf.start_iter(), &self.buf.end_iter());
        self.buf.apply_tag(&self.tags.flash, &self.iter_at(s), &self.iter_at(e));
        let buf = self.buf.clone();
        let tag = self.tags.flash.clone();
        workers::later("flash", ms, move || buf.remove_tag(&tag, &buf.start_iter(), &buf.end_iter()));
    }

    /// Replace snap.text[s:e] with new. In a formatted document only the words that differ are replaced,
    /// so everything else keeps its formatting and paragraphs keep their styles.
    pub fn put_text(&self, snap: &Snap, s: usize, e: usize, new: &str) {
        if self.rich_active() {
            let old = snap.get(s, e);
            let pieces = engine::diff_chunks(old, new);
            if !pieces.is_empty() || old == new {
                for (a, b, rep) in pieces.into_iter().rev() {
                    let (mut i1, mut i2) = (self.iter_at(s + a), self.iter_at(s + b));
                    self.buf.delete(&mut i1, &mut i2);
                    let mut i1 = self.iter_at(s + a);
                    self.buf.insert(&mut i1, &rep);
                }
                return;
            }
        }
        let (mut i1, mut i2) = (self.iter_at(s), self.iter_at(e));
        self.buf.delete(&mut i1, &mut i2);
        let mut i1 = self.iter_at(s);
        self.buf.insert(&mut i1, new);
    }

    pub fn revert_here(&self) {
        let Some((cs, ce, os, oe)) = self.st.borrow().current_change else { return };
        let snap = self.refresh_snapshot();
        let orig = self.st.borrow().original.clone();
        let off = engine::Offsets::new(&orig);
        let cur = snap.get(cs, ce).to_string();
        self.replace(cs, ce, &cur, &engine::slice(&orig, &off, os, oe));
        later(self, "target", 30, |a| a.update_target(true));
    }

    pub fn revert_all(self: &Rc<Self>) {
        if self.busy() || self.st.borrow().original.is_empty() {
            return;
        }
        let me = self.clone();
        if self.mode() == "rich" {
            self.ask("Revert everything", "Go back to the document as it was opened? (This cannot be undone.)", move |yes| {
                if yes {
                    let orig = me.st.borrow().rich_original.clone();
                    if let Some(m) = orig {
                        me.st.borrow_mut().loading = true;
                        me.rich().load(&m);
                        me.st.borrow_mut().loading = false;
                        me.st.borrow_mut().snap_dirty = true;
                        me.st.borrow_mut().dirty = true;
                        me.update_title();
                        me.analyze();
                    }
                }
            });
            return;
        }
        self.ask("Revert everything", "Replace the text with the original version? (You can undo this.)", move |yes| {
            if yes {
                let orig = me.st.borrow().original.clone();
                me.one_step(|| {
                    let (mut a, mut b) = (me.buf.start_iter(), me.buf.end_iter());
                    me.buf.delete(&mut a, &mut b);
                    me.buf.insert(&mut me.buf.start_iter(), &orig);
                });
            }
        });
    }

    pub fn mark_original(&self) {
        let snap = self.refresh_snapshot();
        self.st.borrow_mut().original = snap.text.clone();
        self.analyze();
        self.flash_msg("Current text is now the original for change tracking.", 5000);
    }

    // -- sentences ----------------------------------------------------------------------------

    pub fn rephrase_sentence(&self, more: bool) {
        if self.busy() {
            return;
        }
        if self.mode() == "pdf" {
            self.flash_msg(crate::app::PDF_HINT, 8000);
            return;
        }
        let snap = self.refresh_snapshot();
        let cur = self.st.borrow().sent_target.clone();
        let (s, e) = match cur {
            Some((s, e, t, _)) if more && snap.get(s, e) == t => {
                self.st.borrow_mut().variation += 1;
                (s, e)
            }
            _ => {
                let off = self.selection().map_or_else(|| self.cursor_offset(), |(a, _)| a);
                let Some(span) = self.sentence_at(&snap, off) else {
                    self.flash_msg("Put the cursor inside a sentence first.", 5000);
                    return;
                };
                self.st.borrow_mut().variation = 0;
                span
            }
        };
        let id = {
            let mut st = self.st.borrow_mut();
            st.next_target += 1;
            let id = st.next_target;
            st.sent_target = Some((s, e, snap.get(s, e).to_string(), id));
            id
        };
        let spans = engine::clip(&self.st.borrow().protected, s, e);
        self.panel().set_sentence("loading", vec![]);
        let sentence = snap.get(s, e).to_string();
        let var = self.st.borrow().variation;
        let eng = self.engine.clone();
        let me = self.weak();
        self.lanes.sent.submit(
            move || eng.paraphrase(&sentence, &spans, 5, var),
            move |res| {
                let Some(app) = me.upgrade() else { return };
                if app.st.borrow().sent_target.as_ref().map(|t| t.3) != Some(id) {
                    return;
                }
                match res {
                    Err(err) => {
                        app.st.borrow_mut().sent_items.clear();
                        app.panel().set_sentence("empty", vec![]);
                        app.flash_msg(&format!("Rephrasing failed: {err}"), 5000);
                    }
                    Ok(items) => {
                        app.st.borrow_mut().sent_items = items.clone();
                        let status = if items.is_empty() { "empty" } else { "results" };
                        app.panel().set_sentence(status, items);
                        if !app.engine.has_paraphraser() {
                            app.flash_msg("Tip: the paraphrase model gives much better sentence rewrites (Models menu).", 5000);
                        }
                    }
                }
            },
        );
    }

    pub fn apply_sentence(&self, n: usize) {
        let (Some((s, e, old, _)), items) = (self.st.borrow().sent_target.clone(), self.st.borrow().sent_items.clone()) else { return };
        if self.busy() || n >= items.len() {
            return;
        }
        if self.replace(s, e, &old, &items[n].text) {
            let mut st = self.st.borrow_mut();
            st.sent_target = None;
            st.sent_items.clear();
            drop(st);
            self.panel().set_sentence("idle", vec![]);
        }
    }

    // -- auto rewrite -----------------------------------------------------------------------

    pub fn auto_rewrite(self: &Rc<Self>) {
        if self.busy() {
            return;
        }
        if self.mode() == "pdf" {
            self.flash_msg(crate::app::PDF_HINT, 8000);
            return;
        }
        crate::dialogs::auto_rewrite_dialog(self, self.selection().is_some());
    }

    /// Run auto-rewrite with the dialog's choices: scope (selection | all), strength, phrases, words,
    /// sentences, apply (all | review).
    pub fn run_auto(&self, scope: &str, opts: AutoOptions, review: bool) {
        let snap = self.refresh_snapshot();
        if snap.text.trim().is_empty() {
            return;
        }
        let mut regions = vec![(0, snap.len())];
        if let (true, Some((a, b))) = (scope == "selection", self.selection()) {
            regions = vec![(a, b)];
        }
        self.clear_proposals();
        let flag = Cancel::new();
        {
            let mut st = self.st.borrow_mut();
            st.busy = true;
            st.cancel = flag.clone();
        }
        self.text.set_editable(false);
        self.show_progress("Rewriting…");
        let prot = self.st.borrow().protected.clone();
        let eng = self.engine.clone();
        let started = Instant::now();
        let opts = AutoOptions { lines: self.rich_active(), ..opts };
        let text = snap.text.clone();
        let me = self.weak();
        self.lanes.slow.submit(
            move || {
                let progress = move |k: usize, n: usize| {
                    let el = started.elapsed().as_secs_f64();
                    workers::post(move || with_app(|app| app.auto_progress(k, n, el)));
                };
                eng.auto_rewrite(&text, &regions, &prot, &opts, &progress, &|| flag.is_set())
            },
            move |res| {
                if let Some(app) = me.upgrade() {
                    app.auto_done(&snap, res, review);
                }
            },
        );
    }

    fn auto_progress(&self, k: usize, n: usize, elapsed: f64) {
        if !self.busy() {
            return;
        }
        let mut eta = String::new();
        if k >= 2 {
            let rem = elapsed / k as f64 * (n - k) as f64;
            eta = if rem < 90.0 { format!(" · about {}s left", rem as u64 + 1) } else { format!(" · about {} min left", (rem / 60.0) as u64 + 1) };
        }
        self.set_progress(k, n, &format!("Rewriting sentence {} of {n}{eta}", (k + 1).min(n)));
    }

    fn auto_done(&self, snap: &Rc<Snap>, res: Result<Vec<engine::Edit>, String>, review: bool) {
        self.st.borrow_mut().busy = false;
        self.text.set_editable(true);
        self.hide_progress();
        let edits = match res {
            Err(e) => {
                self.error("Auto-rewrite failed", &e);
                return;
            }
            Ok(e) => e,
        };
        if self.editor_text() != snap.text {
            self.error("Auto-rewrite", "The text changed while rewriting; nothing was applied.");
            return;
        }
        if edits.is_empty() {
            self.flash_msg("Nothing to change at this strength.", 5000);
            return;
        }
        if review {
            let n = self.make_proposals(snap, &edits);
            self.flash_msg(&format!("{n} suggested change{} highlighted — click one to accept or reject it.", if n != 1 { "s" } else { "" }), 9000);
            self.next_prop(Some(0), true);
            return;
        }
        let mut sorted = edits.clone();
        sorted.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        self.one_step(|| {
            for (s, e, new, _) in &sorted {
                self.put_text(snap, *s, *e, new);
            }
        });
        self.st.borrow_mut().snap_dirty = true;
        if !self.settings().show_changes {
            self.w().t_changes.set(true);
            self.set_show_changes(true);
        }
        self.analyze();
        let n = edits.len();
        self.flash_msg(&format!("Rewrote {n} sentence{} — {MOD}+Z undoes the whole pass.", if n != 1 { "s" } else { "" }), 5000);
    }

    // -- review mode: highlighted suggestions accepted one by one ----------------------------------

    fn make_proposals(&self, snap: &Snap, edits: &[engine::Edit]) -> usize {
        self.clear_proposals();
        let mut sorted = edits.to_vec();
        sorted.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        for (s, e, new, whole) in sorted {
            let old = snap.get(s, e).to_string();
            let pieces = if whole { vec![(0, engine::char_len(&old), new.clone())] } else { engine::diff_chunks(&old, &new) };
            let off = engine::Offsets::new(&old);
            for (a, b, rep) in pieces {
                let pid = {
                    let mut st = self.st.borrow_mut();
                    st.next_pid += 1;
                    st.next_pid
                };
                let (i1, i2) = (self.iter_at(s + a), self.iter_at(s + b));
                // marks move with the text, so suggestions survive edits made elsewhere
                let m1 = self.buf.create_mark(None, &i1, false);
                let m2 = self.buf.create_mark(None, &i2, true);
                self.buf.apply_tag(&self.tags.prop, &i1, &i2);
                self.st.borrow_mut().proposals.insert(pid, Proposal { old: engine::slice(&old, &off, a, b), new: rep, start: m1, end: m2 });
            }
        }
        self.update_review();
        self.st.borrow().proposals.len()
    }

    fn prop_range(&self, pid: u64) -> Option<(gtk::TextIter, gtk::TextIter)> {
        let st = self.st.borrow();
        let p = st.proposals.get(&pid)?;
        Some((self.buf.iter_at_mark(&p.start), self.buf.iter_at_mark(&p.end)))
    }

    fn prop_order(&self) -> Vec<u64> {
        let mut v: Vec<(i32, u64)> = {
            let st = self.st.borrow();
            st.proposals.iter().map(|(&pid, p)| (self.buf.iter_at_mark(&p.start).offset(), pid)).collect()
        };
        v.sort();
        v.into_iter().map(|(_, p)| p).collect()
    }

    fn drop_prop(&self, pid: u64) {
        let Some(p) = self.st.borrow_mut().proposals.remove(&pid) else { return };
        let (i1, i2) = (self.buf.iter_at_mark(&p.start), self.buf.iter_at_mark(&p.end));
        if i1.offset() < i2.offset() {
            self.buf.remove_tag(&self.tags.prop, &i1, &i2);
        }
        self.buf.delete_mark(&p.start);
        self.buf.delete_mark(&p.end);
    }

    /// Drop suggestions whose text the user has edited by hand.
    fn prune_props(&self) {
        let pids: Vec<u64> = self.st.borrow().proposals.keys().copied().collect();
        for pid in pids {
            let Some((i1, i2)) = self.prop_range(pid) else { continue };
            let cur = self.buf.slice(&i1, &i2, true).to_string();
            if cur != self.st.borrow().proposals[&pid].old {
                self.drop_prop(pid);
            }
        }
    }

    pub fn clear_proposals(&self) {
        let props: Vec<Proposal> = self.st.borrow_mut().proposals.drain().map(|(_, p)| p).collect();
        for p in props {
            self.buf.delete_mark(&p.start);
            self.buf.delete_mark(&p.end);
        }
        self.buf.remove_tag(&self.tags.prop, &self.buf.start_iter(), &self.buf.end_iter());
        if self.panel.get().is_some() {
            self.update_review();
        }
    }

    fn prop_at(&self, off: usize) -> Option<u64> {
        // the earliest suggestion that touches the position, as in the order of the text
        let st = self.st.borrow();
        st.proposals
            .iter()
            .map(|(&pid, p)| (self.buf.iter_at_mark(&p.start).offset() as usize, self.buf.iter_at_mark(&p.end).offset() as usize, pid))
            .filter(|&(a, b, _)| a <= off && off <= b)
            .min()
            .map(|(_, _, pid)| pid)
    }

    pub fn update_review(&self) {
        if self.st.borrow().proposals.is_empty() {
            self.panel().set_review(None);
            return;
        }
        let pid = self.prop_at(self.cursor_offset());
        let cur = pid.map(|pid| {
            let st = self.st.borrow();
            let p = &st.proposals[&pid];
            let (old, new) = (p.old.clone(), p.new.clone());
            drop(st);
            let pos = self.prop_order().iter().position(|&x| x == pid).unwrap_or(0) + 1;
            (old, new, pos)
        });
        let n = self.st.borrow().proposals.len();
        self.panel().set_review(Some((n, cur)));
    }

    /// Go to the next suggestion after `start` (the cursor when None), wrapping around.
    pub fn next_prop(&self, start: Option<usize>, inclusive: bool) {
        let order = self.prop_order();
        if order.is_empty() {
            self.update_review();
            later(self, "target", 10, |a| a.update_target(true));
            return;
        }
        let start = start.unwrap_or_else(|| self.cursor_offset());
        let pid = order
            .iter()
            .copied()
            .find(|&p| {
                let o = self.prop_range(p).map_or(0, |(a, _)| a.offset() as usize);
                if inclusive { o >= start } else { o > start }
            })
            .unwrap_or(order[0]);
        let Some((i1, i2)) = self.prop_range(pid) else { return };
        self.buf.place_cursor(&i1);
        self.text.scroll_to_iter(&mut i2.clone(), 0.1, false, 0.0, 0.0);
        self.text.scroll_to_iter(&mut i1.clone(), 0.1, false, 0.0, 0.0);
        self.text.grab_focus();
        self.update_target(false);
    }

    pub fn accept_prop(&self) {
        if self.busy() {
            return;
        }
        let Some(pid) = self.prop_at(self.cursor_offset()) else {
            if !self.st.borrow().proposals.is_empty() {
                self.next_prop(None, false);
            }
            return;
        };
        let (old, new) = {
            let st = self.st.borrow();
            (st.proposals[&pid].old.clone(), st.proposals[&pid].new.clone())
        };
        let (i1, i2) = self.prop_range(pid).unwrap();
        let mut next = i1.offset() as usize;
        if self.buf.slice(&i1, &i2, true) != old {
            self.drop_prop(pid);
            self.flash_msg("That text was edited, so its suggestion was dropped.", 5000);
        } else {
            let s = i1.offset() as usize;
            self.one_step(|| {
                let (mut a, mut b) = self.prop_range(pid).unwrap();
                self.buf.delete(&mut a, &mut b);
                let mut a = self.iter_at(s);
                self.buf.insert(&mut a, &new);
            });
            self.drop_prop(pid);
            let end = s + engine::char_len(&new);
            self.flash(s, end, 700);
            self.st.borrow_mut().snap_dirty = true;
            self.analyze();
            next = end;
        }
        self.next_prop(Some(next), true);
    }

    pub fn reject_prop(&self) {
        let Some(pid) = self.prop_at(self.cursor_offset()) else {
            if !self.st.borrow().proposals.is_empty() {
                self.next_prop(None, false);
            }
            return;
        };
        let s = self.prop_range(pid).map_or(0, |(a, _)| a.offset() as usize);
        self.drop_prop(pid);
        self.next_prop(Some(s), true);
    }

    pub fn accept_all_props(&self) {
        if self.busy() || self.st.borrow().proposals.is_empty() {
            return;
        }
        let mut n = 0;
        let order = self.prop_order();
        self.one_step(|| {
            for &pid in order.iter().rev() {
                // back to front keeps earlier positions valid
                let Some((mut a, mut b)) = self.prop_range(pid) else { continue };
                let (old, new) = {
                    let st = self.st.borrow();
                    (st.proposals[&pid].old.clone(), st.proposals[&pid].new.clone())
                };
                if self.buf.slice(&a, &b, true) == old {
                    let s = a.offset() as usize;
                    self.buf.delete(&mut a, &mut b);
                    self.buf.insert(&mut self.iter_at(s), &new);
                    n += 1;
                }
            }
        });
        self.clear_proposals();
        self.st.borrow_mut().snap_dirty = true;
        self.analyze();
        self.flash_msg(&format!("Accepted {n} change{} — {MOD}+Z undoes them all.", if n != 1 { "s" } else { "" }), 5000);
        later(self, "target", 10, |a| a.update_target(true));
    }

    pub fn reject_all_props(&self) {
        let n = self.st.borrow().proposals.len();
        self.clear_proposals();
        self.flash_msg(&format!("Dismissed {n} suggestion{}.", if n != 1 { "s" } else { "" }), 5000);
        later(self, "target", 10, |a| a.update_target(true));
    }

    // -- popups --------------------------------------------------------------------------------

    fn context_menu(&self, x: f64, y: f64) {
        let (bx, by) = self.text.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let Some(it) = self.text.iter_at_location(bx, by) else { return };
        let idx = it.offset() as usize;
        let inside = self.selection().is_some_and(|(a, b)| a <= idx && idx <= b);
        if !inside {
            self.buf.place_cursor(&it);
        }
        self.update_target(false);
        self.show_menu(x as i32, y as i32);
    }

    pub fn popup_at_cursor(&self) {
        if self.mode() == "pdf" {
            return;
        }
        self.update_target(false);
        let loading = self.st.borrow().target.as_ref().is_some_and(|t| t.items.is_none());
        if loading {
            self.st.borrow_mut().popup_pending = true;
            return;
        }
        let it = self.buf.iter_at_mark(&self.buf.get_insert());
        let r = self.text.iter_location(&it);
        let (x, y) = self.text.buffer_to_window_coords(gtk::TextWindowType::Widget, r.x(), r.y() + r.height());
        self.show_menu(x, y + 4);
    }

    fn show_menu(&self, x: i32, y: i32) {
        let pop = gtk::Popover::new();
        pop.set_has_arrow(false);
        pop.set_position(gtk::PositionType::Bottom);
        pop.set_halign(gtk::Align::Start);
        let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let me = self.weak();
        let p2 = pop.clone();
        let add = |label: &str, accel: &str, enabled: bool, f: Box<dyn Fn(&AppRc)>| {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            let l = widgets::label(label, &[]);
            l.set_hexpand(true);
            l.set_max_width_chars(60);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row.append(&l);
            if !accel.is_empty() {
                row.append(&widgets::label(accel, &["muted", "small"]));
            }
            let b = gtk::Button::new();
            b.add_css_class("flat-btn");
            b.add_css_class("compact");
            b.set_child(Some(&row));
            b.set_sensitive(enabled);
            let me = me.clone();
            let p = p2.clone();
            b.connect_clicked(move |_| {
                p.popdown();
                if let Some(app) = me.upgrade() {
                    f(&app);
                }
            });
            col.append(&b);
        };
        let sep = |col: &gtk::Box| {
            let s = widgets::hairline();
            s.set_margin_top(3);
            s.set_margin_bottom(3);
            col.append(&s);
        };
        let t = self.st.borrow().target.clone();
        if let Some(pid) = self.prop_at(self.cursor_offset()) {
            let new = self.st.borrow().proposals[&pid].new.trim().to_string();
            let new = if new.is_empty() { "(remove)".to_string() } else { new };
            add(&format!("Accept: {}", new.chars().take(60).collect::<String>()), "", true, Box::new(|a| a.accept_prop()));
            add("Reject suggestion", "", true, Box::new(|a| a.reject_prop()));
            sep(&col);
        }
        let issue = self.st.borrow().current_issue.clone();
        if let Some(it) = issue {
            for (n, fix) in it.fixes.iter().take(3).enumerate() {
                let f = if fix.trim().is_empty() { "(remove)".to_string() } else { fix.trim().to_string() };
                add(&format!("Fix: {f}"), "grammar", true, Box::new(move |a| a.fix_issue(n)));
            }
            add("Ignore this remark", "", true, Box::new(|a| a.ignore_issue()));
            sep(&col);
        }
        match &t {
            Some(t) if t.kind == "spell" && t.items.is_some() => {
                let items = t.items.as_ref().unwrap();
                if items.is_empty() {
                    add("No spelling suggestions", "", false, Box::new(|_| {}));
                }
                for (i, (s, _, _, _)) in items.iter().take(8).enumerate() {
                    add(&s.text, "spelling", true, Box::new(move |a| a.apply_suggestion(i)));
                }
                sep(&col);
                add(&format!("Add “{}” to dictionary", t.text), "", true, Box::new(|a| a.add_word(false)));
                add("Ignore this word", "", true, Box::new(|a| a.add_word(true)));
                sep(&col);
            }
            Some(t) if t.items.as_ref().is_some_and(|i| !i.is_empty()) => {
                let mut last: Option<(usize, usize)> = None;
                for (i, (s, a, b, _)) in t.items.as_ref().unwrap().iter().take(14).enumerate() {
                    if last.is_some_and(|l| l != (*a, *b)) {
                        sep(&col);
                    }
                    last = Some((*a, *b));
                    add(&s.text, &s.kind, true, Box::new(move |a| a.apply_suggestion(i)));
                }
                sep(&col);
            }
            Some(t) if t.items.is_none() => {
                add("Finding alternatives…", "", false, Box::new(|_| {}));
                sep(&col);
            }
            _ => {}
        }
        add("Cut", "", true, Box::new(|a| a.text.emit_cut_clipboard()));
        add("Copy", "", true, Box::new(|a| a.text.emit_copy_clipboard()));
        add("Paste", "", true, Box::new(|a| a.text.emit_paste_clipboard()));
        sep(&col);
        add("Rephrase sentence", "", true, Box::new(|a| a.rephrase_sentence(false)));
        if self.st.borrow().current_change.is_some() {
            add("Revert to original", "", true, Box::new(|a| a.revert_here()));
        }
        pop.set_child(Some(&col));
        pop.set_parent(&self.text);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x, y, 1, 1)));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

use crossterm::{
    cursor,
    event::{KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::{Print, ResetColor, SetForegroundColor},
    terminal::{
        self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
        SetTitle,
    },
    ExecutableCommand, QueueableCommand, Result,
};

use std::{
    io::{stdout, Write},
    iter, thread,
    time::Duration,
};

use crate::{
    action::{ActionKind, ActionResult, ActionTask},
    application::{ActionFuture, Application},
    input::{self, Event},
    scroll_view::ScrollView,
    select::{select, Entry},
    tui_util::{show_header, Header, HeaderKind, TerminalSize, ENTRY_COLOR},
};

const BIN_NAME: &'static str = env!("CARGO_PKG_NAME");
const VERSION: &'static str = env!("CARGO_PKG_VERSION");

pub fn show_tui(mut app: Application) {
    let stdout = stdout();
    let stdout = stdout.lock();
    let mut tui = Tui::new(stdout);
    tui.show(&mut app).unwrap();
}

enum HandleChordResult {
    Handled,
    Unhandled,
    Quit,
}

struct Tui<W>
where
    W: Write,
{
    previous_action_kind: ActionKind,
    current_action_kind: ActionKind,
    current_key_chord: Vec<char>,

    write: W,
    terminal_size: TerminalSize,
    scroll_view: ScrollView,
}

impl<W> Tui<W>
where
    W: Write,
{
    fn new(write: W) -> Self {
        Tui {
            previous_action_kind: ActionKind::Quit,
            current_action_kind: ActionKind::Quit,
            current_key_chord: Vec::new(),
            write,
            terminal_size: Default::default(),
            scroll_view: Default::default(),
        }
    }

    fn show_header(
        &mut self,
        app: &Application,
        kind: HeaderKind,
    ) -> Result<()> {
        let header = Header {
            action_name: self.current_action_kind.name(),
            directory_name: app.version_control.get_root(),
        };
        show_header(&mut self.write, header, kind, self.terminal_size)
    }

    fn show_select_ui(
        &mut self,
        app: &Application,
        entries: &mut [Entry],
    ) -> Result<bool> {
        self.show_header(app, HeaderKind::Waiting)?;
        select(&mut self.write, entries)
    }

    fn show_action(
        &mut self,
        app: &mut Application,
        task: Box<dyn ActionTask>,
    ) -> Result<()> {
        app.run_action(ActionFuture {
            kind: self.current_action_kind,
            task,
        });
        let result = app.get_cached_action_result(self.current_action_kind);
        self.show_result(app, result)
    }

    fn show_empty_entries(&mut self, app: &Application) -> Result<()> {
        self.show_header(app, HeaderKind::Error)?;
        self.write.queue(Print("nothing to select"))?;
        Ok(())
    }

    fn show_previous_action_result(&mut self, app: &Application) -> Result<()> {
        self.current_action_kind = self.previous_action_kind;
        let result = app.get_cached_action_result(self.current_action_kind);
        self.show_result(app, result)
    }

    fn action_context<F>(
        &mut self,
        action: ActionKind,
        callback: F,
    ) -> Result<HandleChordResult>
    where
        F: FnOnce(&mut Self) -> Result<()>,
    {
        self.previous_action_kind = self.current_action_kind;
        self.current_action_kind = action;
        callback(self).map(|_| HandleChordResult::Handled)
    }

    fn previous_target<'a>(&self, app: &'a Application) -> Option<&'a str> {
        let previous_result =
            app.get_cached_action_result(self.previous_action_kind);
        if !previous_result.success {
            return None;
        }

        self.scroll_view
            .cursor()
            .and_then(|c| previous_result.output.lines().nth(c))
            .and_then(|l| self.previous_action_kind.parse_target(l))
    }

    fn show(&mut self, app: &mut Application) -> Result<()> {
        execute!(
            self.write,
            SetTitle(app.version_control.get_root()),
            EnterAlternateScreen,
            cursor::Hide
        )?;
        terminal::enable_raw_mode()?;

        self.write.flush()?;
        self.terminal_size = TerminalSize::get()?;

        {
            self.current_action_kind = ActionKind::Help;
            let help = self.show_help(app)?;
            self.show_result(app, &help)?;
            self.show_current_key_chord()?;
            self.write.flush()?;

            app.set_cached_action_result(ActionKind::Help, help);
        }

        loop {
            if app.poll_and_check_action(self.current_action_kind) {
                let result =
                    app.get_cached_action_result(self.current_action_kind);
                self.show_result(app, result)?;
                self.write.flush()?;
            }

            match input::poll_event() {
                Event::Resize(terminal_size) => {
                    self.terminal_size = terminal_size;
                    let result =
                        app.get_cached_action_result(self.current_action_kind);
                    self.show_result(app, result)?;
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Esc, ..
                })
                | Event::Key(KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                }) => {
                    let esc_key_event = KeyEvent {
                        code: KeyCode::Esc,
                        modifiers: KeyModifiers::NONE,
                    };

                    if self.scroll_view.update(
                        &mut self.write,
                        esc_key_event,
                        self.terminal_size,
                    )? {
                        self.write.flush()?;
                        continue;
                    }

                    if self.current_key_chord.len() == 0 {
                        break;
                    }

                    self.current_key_chord.clear();
                    self.show_current_key_chord()?;
                    self.write.flush()?;
                }
                Event::Key(key_event) => {
                    if self.scroll_view.update(
                        &mut self.write,
                        key_event,
                        self.terminal_size,
                    )? {
                        self.write.flush()?;
                        continue;
                    }

                    if let Some(c) = input::key_to_char(key_event) {
                        self.current_key_chord.push(c);
                    }

                    match self.handle_key_chord(app)? {
                        HandleChordResult::Handled => {
                            self.current_key_chord.clear()
                        }
                        HandleChordResult::Unhandled => (),
                        HandleChordResult::Quit => break,
                    }

                    self.show_current_key_chord()?;
                    self.write.flush()?;
                }
                _ => (),
            }

            thread::sleep(Duration::from_millis(20));
        }

        execute!(self.write, ResetColor, cursor::Show)?;
        terminal::disable_raw_mode()?;
        self.write.execute(LeaveAlternateScreen)?;
        Ok(())
    }

    fn handle_key_chord(
        &mut self,
        app: &mut Application,
    ) -> Result<HandleChordResult> {
        match &self.current_key_chord[..] {
            ['q'] => Ok(HandleChordResult::Quit),
            ['h'] => {
                self.current_action_kind = ActionKind::Help;
                let help = self.show_help(app)?;
                self.show_result(app, &help)?;
                Ok(HandleChordResult::Handled)
            }
            ['s'] => self.action_context(ActionKind::Status, |s| {
                let action = app.version_control.status();
                s.show_action(app, action)
            }),
            ['l'] => self.action_context(ActionKind::Log, |s| {
                let action =
                    app.version_control.log(s.terminal_size.height as usize);
                s.show_action(app, action)
            }),
            ['L'] => Ok(HandleChordResult::Unhandled),
            ['L', 'C'] => self.action_context(ActionKind::LogCount, |s| {
                if let Some(input) =
                    s.handle_input(app, "logs to show", None)?
                {
                    if let Ok(count) = input.trim().parse() {
                        let action = app.version_control.log(count);
                        s.show_action(app, action)
                    } else {
                        s.show_header(app, HeaderKind::Error)?;
                        queue!(
                            s.write,
                            Print("could not parse a number from "),
                            Print(input)
                        )
                    }
                } else {
                    s.show_previous_action_result(app)
                }
            }),
            ['e'] => Ok(HandleChordResult::Unhandled),
            ['e', 'e'] => {
                self.action_context(ActionKind::CurrentFullRevision, |s| {
                    let action = app.version_control.current_export();
                    s.show_action(app, action)
                })
            }
            ['d'] => Ok(HandleChordResult::Unhandled),
            ['d', 'd'] => {
                self.action_context(ActionKind::CurrentDiffAll, |s| {
                    let action = app.version_control.current_diff_all();
                    s.show_action(app, action)
                })
            }
            ['d', 's'] => {
                self.action_context(ActionKind::CurrentDiffSelected, |s| {
                    match app.version_control.get_current_changed_files() {
                        Ok(mut entries) => {
                            if entries.len() == 0 {
                                s.show_empty_entries(app)
                            } else if s.show_select_ui(app, &mut entries[..])? {
                                let action = app
                                    .version_control
                                    .current_diff_selected(&entries);
                                s.show_action(app, action)
                            } else {
                                s.show_previous_action_result(app)
                            }
                        }
                        Err(error) => {
                            s.show_result(app, &ActionResult::from_err(error))
                        }
                    }
                })
            }
            ['D'] => Ok(HandleChordResult::Unhandled),
            ['D', 'C'] => {
                self.action_context(ActionKind::RevisionChanges, |s| {
                    if let Some(input) = s.handle_input(
                        app,
                        "show changes from",
                        s.previous_target(app),
                    )? {
                        let action =
                            app.version_control.revision_changes(input.trim());
                        s.show_action(app, action)
                    } else {
                        s.show_previous_action_result(app)
                    }
                })
            }
            ['D', 'D'] => {
                self.action_context(ActionKind::RevisionDiffAll, |s| {
                    if let Some(input) = s.handle_input(
                        app,
                        "show diff from",
                        s.previous_target(app),
                    )? {
                        let action =
                            app.version_control.revision_diff_all(input.trim());
                        s.show_action(app, action)
                    } else {
                        s.show_previous_action_result(app)
                    }
                })
            }
            ['D', 'S'] => {
                self.action_context(ActionKind::RevisionDiffSelected, |s| {
                    if let Some(input) = s.handle_input(
                        app,
                        "show diff from",
                        s.previous_target(app),
                    )? {
                        match app
                            .version_control
                            .get_revision_changed_files(input.trim())
                        {
                            Ok(mut entries) => {
                                if entries.len() == 0 {
                                    s.show_empty_entries(app)
                                } else if s
                                    .show_select_ui(app, &mut entries[..])?
                                {
                                    let action = app
                                        .version_control
                                        .revision_diff_selected(
                                            input.trim(),
                                            &entries,
                                        );
                                    s.show_action(app, action)
                                } else {
                                    s.show_previous_action_result(app)
                                }
                            }
                            Err(error) => s.show_result(
                                app,
                                &ActionResult::from_err(error),
                            ),
                        }
                    } else {
                        s.show_previous_action_result(app)
                    }
                })
            }
            ['c'] => Ok(HandleChordResult::Unhandled),
            ['c', 'c'] => self.action_context(ActionKind::CommitAll, |s| {
                if let Some(input) =
                    s.handle_input(app, "commit message", None)?
                {
                    let action = app.version_control.commit_all(input.trim());
                    s.show_action(app, action)
                } else {
                    s.show_previous_action_result(app)
                }
            }),
            ['c', 's'] => {
                self.action_context(ActionKind::CommitSelected, |s| {
                    match app.version_control.get_current_changed_files() {
                        Ok(mut entries) => {
                            if entries.len() == 0 {
                                s.show_empty_entries(app)
                            } else if s.show_select_ui(app, &mut entries[..])? {
                                s.show_header(app, HeaderKind::Waiting)?;
                                if let Some(input) =
                                    s.handle_input(app, "commit message", None)?
                                {
                                    let action =
                                        app.version_control.commit_selected(
                                            input.trim(),
                                            &entries,
                                        );
                                    s.show_action(app, action)
                                } else {
                                    s.show_previous_action_result(app)
                                }
                            } else {
                                s.show_previous_action_result(app)
                            }
                        }
                        Err(error) => {
                            s.show_result(app, &ActionResult::from_err(error))
                        }
                    }
                })
            }
            ['u'] => self.action_context(ActionKind::Update, |s| {
                if let Some(input) =
                    s.handle_input(app, "update to", s.previous_target(app))?
                {
                    let action = app.version_control.update(input.trim());
                    s.show_action(app, action)
                } else {
                    s.show_previous_action_result(app)
                }
            }),
            ['m'] => self.action_context(ActionKind::Merge, |s| {
                if let Some(input) =
                    s.handle_input(app, "merge with", s.previous_target(app))?
                {
                    let action = app.version_control.merge(input.trim());
                    s.show_action(app, action)
                } else {
                    s.show_previous_action_result(app)
                }
            }),
            ['R'] => Ok(HandleChordResult::Unhandled),
            ['R', 'A'] => self.action_context(ActionKind::RevertAll, |s| {
                let action = app.version_control.revert_all();
                s.show_action(app, action)
            }),
            ['r'] => Ok(HandleChordResult::Unhandled),
            ['r', 's'] => {
                self.action_context(ActionKind::RevertSelected, |s| {
                    match app.version_control.get_current_changed_files() {
                        Ok(mut entries) => {
                            if entries.len() == 0 {
                                s.show_empty_entries(app)
                            } else if s.show_select_ui(app, &mut entries[..])? {
                                let action = app
                                    .version_control
                                    .revert_selected(&entries);
                                s.show_action(app, action)
                            } else {
                                s.show_previous_action_result(app)
                            }
                        }
                        Err(error) => {
                            s.show_result(app, &ActionResult::from_err(error))
                        }
                    }
                })
            }
            ['r', 'r'] => {
                self.action_context(ActionKind::UnresolvedConflicts, |s| {
                    let action = app.version_control.conflicts();
                    s.show_action(app, action)
                })
            }
            ['r', 'o'] => {
                self.action_context(ActionKind::MergeTakingOther, |s| {
                    let action = app.version_control.take_other();
                    s.show_action(app, action)
                })
            }
            ['r', 'l'] => {
                self.action_context(ActionKind::MergeTakingLocal, |s| {
                    let action = app.version_control.take_local();
                    s.show_action(app, action)
                })
            }
            ['f'] => self.action_context(ActionKind::Fetch, |s| {
                let action = app.version_control.fetch();
                s.show_action(app, action)
            }),
            ['p'] => self.action_context(ActionKind::Pull, |s| {
                let action = app.version_control.pull();
                s.show_action(app, action)
            }),
            ['P'] => self.action_context(ActionKind::Push, |s| {
                let action = app.version_control.push();
                s.show_action(app, action)
            }),
            ['t'] => Ok(HandleChordResult::Unhandled),
            ['t', 'n'] => self.action_context(ActionKind::NewTag, |s| {
                if let Some(input) =
                    s.handle_input(app, "new tag name", None)?
                {
                    let action = app.version_control.create_tag(input.trim());
                    s.show_action(app, action)
                } else {
                    s.show_previous_action_result(app)
                }
            }),
            ['b'] => Ok(HandleChordResult::Unhandled),
            ['b', 'b'] => self.action_context(ActionKind::ListBranches, |s| {
                let action = app.version_control.list_branches();
                s.show_action(app, action)
            }),
            ['b', 'n'] => self.action_context(ActionKind::NewBranch, |s| {
                if let Some(input) =
                    s.handle_input(app, "new branch name", None)?
                {
                    let action =
                        app.version_control.create_branch(input.trim());
                    s.show_action(app, action)
                } else {
                    s.show_previous_action_result(app)
                }
            }),
            ['b', 'd'] => self.action_context(ActionKind::DeleteBranch, |s| {
                if let Some(input) = s.handle_input(
                    app,
                    "branch to delete",
                    s.previous_target(app),
                )? {
                    let action = app.version_control.close_branch(input.trim());
                    s.show_action(app, action)
                } else {
                    s.show_previous_action_result(app)
                }
            }),
            ['x'] => self.action_context(ActionKind::CustomAction, |s| {
                if app.custom_actions.len() > 0 {
                    s.show_header(app, HeaderKind::Ok)?;
                    for c in &app.custom_actions {
                        s.write
                            .queue(SetForegroundColor(ENTRY_COLOR))?
                            .queue(Print(&c.shortcut))?
                            .queue(ResetColor)?
                            .queue(Print('\t'))?
                            .queue(Print(&c.command))?;
                        for a in &c.args {
                            s.write.queue(Print(' '))?.queue(Print(a))?;
                        }
                        s.write.queue(cursor::MoveToNextLine(1))?;
                    }
                    s.handle_custom_action(app)?;
                    s.current_key_chord.clear();
                } else {
                    s.show_header(app, HeaderKind::Error)?;
                    queue!(
                        s.write,
                        ResetColor,
                        Print("no commands available"),
                        cursor::MoveToNextLine(2),
                        Print(concat!(
                            "create custom actions by placing them inside '.",
                            env!("CARGO_PKG_NAME"),
                            "/custom_actions.txt'"
                        )),
                    )?;
                }
                Ok(())
            }),
            _ => Ok(HandleChordResult::Handled),
        }
    }

    fn handle_custom_action(&mut self, app: &mut Application) -> Result<()> {
        self.current_key_chord.clear();
        self.write.queue(cursor::SavePosition)?;

        'outer: loop {
            self.write.flush()?;
            match input::poll_event() {
                Event::Resize(terminal_size) => {
                    self.terminal_size = terminal_size;
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Esc, ..
                })
                | Event::Key(KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                }) => {
                    return self.show_previous_action_result(app);
                }
                Event::Key(key_event) => {
                    if let Some(c) = input::key_to_char(key_event) {
                        self.current_key_chord.push(c);
                    }
                    for action in &app.custom_actions {
                        if action
                            .shortcut
                            .chars()
                            .zip(
                                self.current_key_chord
                                    .iter()
                                    .map(|c| *c)
                                    .chain(iter::repeat('\0')),
                            )
                            .all(|(a, b)| a == b)
                        {
                            self.write
                                .queue(cursor::RestorePosition)?
                                .queue(cursor::MoveToNextLine(2))?
                                .queue(SetForegroundColor(ENTRY_COLOR))?
                                .queue(Print(&action.command))?
                                .queue(ResetColor)?;
                            for arg in &action.args {
                                self.write
                                    .queue(Print(' '))?
                                    .queue(Print(arg))?;
                            }
                            self.write.queue(cursor::MoveToNextLine(2))?;

                            let result =
                                action.execute(app.version_control.get_root());
                            self.show_result(app, &result)?;
                            return Ok(());
                        }
                    }
                    self.show_current_key_chord()?;

                    for action in &app.custom_actions {
                        if action
                            .shortcut
                            .chars()
                            .zip(&self.current_key_chord)
                            .all(|(a, b)| a == *b)
                        {
                            continue 'outer;
                        }
                    }

                    self.show_header(app, HeaderKind::Error)?;
                    self.write.queue(Print("no match found"))?;
                    return Ok(());
                }
                _ => (),
            }
        }
    }

    fn handle_input(
        &mut self,
        app: &Application,
        prompt: &str,
        initial: Option<&str>,
    ) -> Result<Option<String>> {
        self.show_header(app, HeaderKind::Waiting)?;
        execute!(
            self.write,
            SetForegroundColor(ENTRY_COLOR),
            Print(prompt),
            ResetColor,
            cursor::MoveToNextLine(1),
            cursor::Show,
        )?;

        let initial = if let Some(initial) = initial {
            initial
        } else {
            ""
        };
        let res = match input::read_line(initial) {
            Ok(line) => {
                if line.len() > 0 {
                    Some(line)
                } else {
                    None
                }
            }
            Err(_error) => None,
        };
        self.write.execute(cursor::Hide)?;
        Ok(res)
    }

    fn show_result(
        &mut self,
        app: &Application,
        result: &ActionResult,
    ) -> Result<()> {
        if app.has_pending_action_of_type(self.current_action_kind) {
            self.show_header(app, HeaderKind::Waiting)?;
        } else if result.success {
            self.show_header(app, HeaderKind::Ok)?;
        } else {
            self.show_header(app, HeaderKind::Error)?;
        }

        self.scroll_view.set_content(
            &result.output[..],
            self.current_action_kind,
            self.terminal_size,
        );
        self.scroll_view
            .draw_content(&mut self.write, self.terminal_size)
    }

    fn show_current_key_chord(&mut self) -> Result<()> {
        let TerminalSize { width, height } = self.terminal_size;
        queue!(
            self.write,
            cursor::MoveTo(
                width - self.current_key_chord.len() as u16,
                height - 1
            ),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(ENTRY_COLOR),
        )?;
        for c in &self.current_key_chord {
            self.write.queue(Print(c))?;
        }
        self.write.queue(ResetColor)?;
        Ok(())
    }

    fn show_help(&mut self, app: &Application) -> Result<ActionResult> {
        let mut write = Vec::with_capacity(1024);

        queue!(
            &mut write,
            Print(BIN_NAME),
            Print(' '),
            Print(VERSION),
            cursor::MoveToNextLine(2),
        )?;

        if let Ok(version) = app.version_control.version() {
            queue!(&mut write, Print(version), cursor::MoveToNextLine(2))?;
        }

        write
            .queue(Print("press a key and peform an action"))?
            .queue(cursor::MoveToNextLine(2))?;

        Self::show_help_action(&mut write, "h", ActionKind::Help)?;
        Self::show_help_action(&mut write, "q", ActionKind::Quit)?;

        write.queue(cursor::MoveToNextLine(1))?;

        Self::show_help_action(&mut write, "s", ActionKind::Status)?;
        Self::show_help_action(&mut write, "l", ActionKind::Log)?;
        Self::show_help_action(&mut write, "LC", ActionKind::LogCount)?;

        Self::show_help_action(
            &mut write,
            "ee",
            ActionKind::CurrentFullRevision,
        )?;
        Self::show_help_action(&mut write, "dd", ActionKind::CurrentDiffAll)?;
        Self::show_help_action(
            &mut write,
            "ds",
            ActionKind::CurrentDiffSelected,
        )?;
        Self::show_help_action(&mut write, "DC", ActionKind::RevisionChanges)?;
        Self::show_help_action(&mut write, "DD", ActionKind::RevisionDiffAll)?;
        Self::show_help_action(
            &mut write,
            "DS",
            ActionKind::RevisionDiffSelected,
        )?;

        write.queue(cursor::MoveToNextLine(1))?;

        Self::show_help_action(&mut write, "cc", ActionKind::CommitAll)?;
        Self::show_help_action(&mut write, "cs", ActionKind::CommitSelected)?;
        Self::show_help_action(&mut write, "u", ActionKind::Update)?;
        Self::show_help_action(&mut write, "m", ActionKind::Merge)?;
        Self::show_help_action(&mut write, "RA", ActionKind::RevertAll)?;
        Self::show_help_action(&mut write, "rs", ActionKind::RevertSelected)?;

        write.queue(cursor::MoveToNextLine(1))?;

        Self::show_help_action(
            &mut write,
            "rr",
            ActionKind::UnresolvedConflicts,
        )?;
        Self::show_help_action(&mut write, "ro", ActionKind::MergeTakingOther)?;
        Self::show_help_action(&mut write, "rl", ActionKind::MergeTakingLocal)?;

        write.queue(cursor::MoveToNextLine(1))?;

        Self::show_help_action(&mut write, "f", ActionKind::Fetch)?;
        Self::show_help_action(&mut write, "p", ActionKind::Pull)?;
        Self::show_help_action(&mut write, "P", ActionKind::Push)?;

        write.queue(cursor::MoveToNextLine(1))?;

        Self::show_help_action(&mut write, "tn", ActionKind::NewTag)?;

        write.queue(cursor::MoveToNextLine(1))?;

        Self::show_help_action(&mut write, "bb", ActionKind::ListBranches)?;
        Self::show_help_action(&mut write, "bn", ActionKind::NewBranch)?;
        Self::show_help_action(&mut write, "bd", ActionKind::DeleteBranch)?;

        write.queue(cursor::MoveToNextLine(1))?;

        Self::show_help_action(&mut write, "x", ActionKind::CustomAction)?;

        write.flush()?;
        Ok(ActionResult::from_ok(String::from_utf8(write)?))
    }

    fn show_help_action<HW>(
        write: &mut HW,
        shortcut: &str,
        action: ActionKind,
    ) -> Result<()>
    where
        HW: Write,
    {
        queue!(
            write,
            SetForegroundColor(ENTRY_COLOR),
            Print('\t'),
            Print(shortcut),
            ResetColor,
            Print('\t'),
            Print('\t'),
            Print(action.name()),
            cursor::MoveToNextLine(1),
        )
    }
}

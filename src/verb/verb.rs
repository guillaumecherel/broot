use {
    super::*,
    crate::{
        app::{Selection, SelInfo, SelectionType},
        errors::ConfError,
        keys,
        path::{self, PathAnchor},
    },
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    std::path::PathBuf,
};

/// what makes a verb.
///
/// Verbs are the engines of broot commands, and apply
/// - to the selected file (if user-defined, then must contain {file}, {parent} or {directory})
/// - to the current app state
/// There are two types of verbs executions:
/// - external programs or commands (cd, mkdir, user defined commands, etc.)
/// - internal behaviors (focusing a path, going back, showing the help, etc.)
/// Some verbs are builtins, some other ones are created by configuration.
/// Both builtins and configured vers can be internal or external based.
#[derive(Debug)]
pub struct Verb {
    /// names (like "cd", "focus", "focus_tab", "c") by which
    /// a verb can be called.
    /// Can be empty if the verb is only called with a key shortcut.
    /// Right now there's no way for it to contain more than 2 elements
    /// but this may change.
    pub names: Vec<String>,

    /// key shortcuts
    pub keys: Vec<KeyEvent>,

    /// description of the optional keyboard key(s) triggering that verb
    pub keys_desc: String,

    /// how the input must be checked and interpreted
    /// Can be empty if the verb is only called with a key shortcut.
    pub invocation_parser: Option<InvocationParser>,

    /// how the verb will be executed
    pub execution: VerbExecution,

    /// a description
    pub description: VerbDescription,

    /// the type of selection this verb applies to
    pub selection_condition: SelectionType,

    /// whether the verb needs a selection
    pub needs_selection: bool,

    /// whether we need to have a secondary panel for execution
    /// (which is the case when the execution pattern has {other-panel-file})
    pub needs_another_panel: bool,

    /// if true (default) verbs are directly executed when
    /// triggered with a keyboard shortcut
    pub auto_exec: bool,
}

impl Verb {

    pub fn new(
        invocation_str: Option<&str>,
        execution: VerbExecution,
        description: VerbDescription,
    ) -> Result<Self, ConfError> {
        let invocation_parser = invocation_str.map(InvocationParser::new).transpose()?;
        let mut names = Vec::new();
        if let Some(ref invocation_parser) = invocation_parser {
            names.push(invocation_parser.name().to_string());
        }
        let (
            needs_selection,
            needs_another_panel,
        ) = match &execution {
            VerbExecution::Internal(ie) => (
                ie.needs_selection(),
                false,
            ),
            VerbExecution::External(ee) => (
                ee.exec_pattern.has_selection_group(),
                ee.exec_pattern.has_other_panel_group(),
            ),
            VerbExecution::Sequence(se) => (
                se.sequence.has_selection_group(),
                se.sequence.has_other_panel_group(),
            )
        };
        Ok(Self {
            names,
            keys: Vec::new(),
            keys_desc: "".to_string(),
            invocation_parser,
            execution,
            description,
            selection_condition: SelectionType::Any,
            needs_selection,
            needs_another_panel,
            auto_exec: true,
        })
    }
    fn update_key_desc(&mut self) {
        self.keys_desc = self
            .keys
            .iter()
            .map(|&k| keys::key_event_desc(k))
            .collect::<Vec<String>>() // no way to join an iterator today ?
            .join(", ");
    }
    pub fn with_key(mut self, key: KeyEvent) -> Self {
        self.keys.push(key);
        self.update_key_desc();
        self
    }
    pub fn add_keys(&mut self, keys: Vec<KeyEvent>) {
        for key in keys {
            self.keys.push(key);
        }
        self.update_key_desc();
    }
    pub fn with_alt_key(self, chr: char) -> Self {
        self.with_key(KeyEvent {
            code: KeyCode::Char(chr),
            modifiers: KeyModifiers::ALT,
        })
    }
    pub fn with_control_key(self, chr: char) -> Self {
        self.with_key(KeyEvent {
            code: KeyCode::Char(chr),
            modifiers: KeyModifiers::CONTROL,
        })
    }
    pub fn with_char_key(self, chr: char) -> Self {
        self.with_key(KeyEvent {
            code: KeyCode::Char(chr),
            modifiers: KeyModifiers::NONE,
        })
    }
    pub fn with_description(mut self, description: &str) -> Self {
        self.description = VerbDescription::from_text(description.to_string());
        self
    }
    pub fn with_shortcut(mut self, shortcut: &str) -> Self {
        self.names.push(shortcut.to_string());
        self
    }
    pub fn with_stype(mut self, stype: SelectionType) -> Self {
        self.selection_condition = stype;
        self
    }
    pub fn needing_another_panel(mut self) -> Self {
        self.needs_another_panel = true;
        self
    }
    pub fn with_auto_exec(mut self, b: bool) -> Self {
        self.auto_exec = b;
        self
    }

    /// Assuming the verb has been matched, check whether the arguments
    /// are OK according to the regex. Return none when there's no problem
    /// and return the error to display if arguments don't match.
    pub fn check_args(
        &self,
        sel_info: &SelInfo<'_>,
        invocation: &VerbInvocation,
        other_path: &Option<PathBuf>,
    ) -> Option<String> {
        match sel_info {
            SelInfo::None => self.check_sel_args(None, invocation, other_path),
            SelInfo::One(sel) => self.check_sel_args(Some(*sel), invocation, other_path),
            SelInfo::More(stage) => {
                stage.paths().iter()
                    .filter_map(|path| {
                        let sel = Selection {
                            path,
                            line: 0,
                            stype: SelectionType::from(path),
                            is_exe: false,
                        };
                        self.check_sel_args(Some(sel), invocation, other_path)
                    })
                    .next()
            }
        }
    }

    fn check_sel_args(
        &self,
        sel: Option<Selection<'_>>,
        invocation: &VerbInvocation,
        other_path: &Option<PathBuf>,
    ) -> Option<String> {
        if self.needs_selection && sel.is_none() {
            Some("This verb needs a selection".to_string())
        } else if self.needs_another_panel && other_path.is_none() {
            Some("This verb needs exactly two panels".to_string())
        } else if let Some(ref parser) = self.invocation_parser {
            parser.check_args(invocation, other_path)
        } else if invocation.args.is_some() {
            Some("This verb doesn't take arguments".to_string())
        } else {
            None
        }
    }

    pub fn get_status_markdown(
        &self,
        sel_info: SelInfo<'_>,
        other_path: &Option<PathBuf>,
        invocation: &VerbInvocation,
    ) -> String {
        let name = self.names.get(0).unwrap_or(&invocation.name);

        // there's one special case: the ̀ :focus` internal. As long
        // as no other internal takes args, and no other verb can
        // have an optional argument, I don't try to build a
        // generic behavior for internal optionaly taking args and
        // thus I hardcode the test here.
        if let VerbExecution::Internal(internal_exec) = &self.execution {
            if internal_exec.internal == Internal::focus {
                if let Some(sel) = sel_info.one_sel() {
                    let arg = invocation.args.as_ref().or_else(|| internal_exec.arg.as_ref());
                    let pb;
                    let arg_path = if let Some(arg) = arg {
                        pb = path::path_from(sel.path, PathAnchor::Unspecified, arg);
                        &pb
                    } else {
                        sel.path
                    };
                    return format!("Hit *enter* to {} `{}`", name, arg_path.to_string_lossy());
                } else {
                    return "You can't focus without selection".to_string();
                }
            }
            // TODO check that before
        }

        let builder = || {
            ExecutionStringBuilder::from_invocation(
                &self.invocation_parser,
                sel_info,
                other_path,
                &invocation.args,
            )
        };
        if let VerbExecution::Sequence(seq_ex) = &self.execution {
            let exec_desc = builder().shell_exec_string(
                &ExecPattern::from_string(&seq_ex.sequence.raw)
            );
            format!("Hit *enter* to **{}**: `{}`", name, &exec_desc)
        } else if let VerbExecution::External(external_exec) = &self.execution {
            let exec_desc = builder().shell_exec_string(&external_exec.exec_pattern);
            format!("Hit *enter* to **{}**: `{}`", name, &exec_desc)
        } else if self.description.code {
            format!("Hit *enter* to **{}**: `{}`", name, &self.description.content)
        } else {
            format!("Hit *enter* to **{}**: {}", name, &self.description.content)
        }
    }

    /// in case the verb take only one argument of type path, return
    /// the selection type of this unique argument
    pub fn get_arg_selection_type(&self) -> Option<SelectionType> {
        self.invocation_parser
            .as_ref()
            .and_then(|parser| parser.arg_selection_type)
    }

    pub fn get_arg_anchor(&self) -> PathAnchor {
        self.invocation_parser
            .as_ref()
            .map_or(PathAnchor::Unspecified, |parser| parser.arg_anchor)
    }

    pub fn get_internal(&self) -> Option<Internal> {
        match &self.execution {
            VerbExecution::Internal(internal_exec) => Some(internal_exec.internal),
            _ => None,
        }
    }

    pub fn is_internal(&self, internal: Internal) -> bool {
        self.get_internal() == Some(internal)
    }

    pub fn is_sequence(&self) -> bool {
        matches!(self.execution, VerbExecution::Sequence(_))
    }
}

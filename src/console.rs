use bevy::ecs::query::FilteredAccessSet;
use bevy::ecs::resource::Resource;
use bevy::ecs::{
    change_detection::Tick,
    system::{ScheduleSystem, SystemMeta, SystemParam},
    world::unsafe_world_cell::UnsafeWorldCell,
};
use bevy::platform::hash::FixedState;
use bevy::{input::keyboard::KeyboardInput, prelude::*};
use bevy_egui::egui::{self, TextEdit};
use bevy_egui::egui::{Context, Id};
use bevy_egui::egui::{text::LayoutJob, text_selection::CCursorRange};
use bevy_egui::{
    EguiContexts,
    egui::{Color32, FontId, TextFormat, epaint::text::cursor::CCursor},
};
use clap::{CommandFactory, FromArgMatches};
use core::str;
use shlex::Shlex;
use std::collections::{BTreeMap, VecDeque};
use std::hash::BuildHasher;
use std::marker::PhantomData;
use std::mem;
use trie_rs::Trie;

use crate::{
    ConsoleSet,
    color::{TextFormattingOverride, parse_ansi_styled_str},
};

type ConsoleCommandEnteredReaderSystemParam =
    MessageReader<'static, 'static, ConsoleCommandEntered>;

type PrintConsoleLineWriterSystemParam = MessageWriter<'static, PrintConsoleLine>;

/// A super-trait for command like structures
pub trait Command: NamedCommand + CommandFactory + FromArgMatches + Sized + Resource {}
impl<T: NamedCommand + CommandFactory + FromArgMatches + Sized + Resource> Command for T {}

/// Trait used to allow uniquely identifying commands at compile time
pub trait NamedCommand {
    /// Return the unique command identifier (same as the command "executable")
    fn name() -> &'static str;
}

/// Executed parsed console command.
///
/// Used to capture console commands which implement [`CommandName`], [`CommandArgs`] & [`CommandHelp`].
/// These can be easily implemented with the [`ConsoleCommand`](bevy_console_derive::ConsoleCommand) derive macro.
///
/// # Example
///
/// ```
/// # use bevy_console::ConsoleCommand;
/// # use clap::Parser;
/// /// Prints given arguments to the console.
/// #[derive(Parser, ConsoleCommand)]
/// #[command(name = "log")]
/// struct LogCommand {
///     /// Message to print
///     msg: String,
///     /// Number of times to print message
///     num: Option<i64>,
/// }
///
/// fn log_command(mut log: ConsoleCommand<LogCommand>) {
///     if let Some(Ok(LogCommand { msg, num })) = log.take() {
///         log.ok();
///     }
/// }
/// ```
pub struct ConsoleCommand<'w, T> {
    command: Option<Result<T, clap::Error>>,
    console_line: MessageWriter<'w, PrintConsoleLine>,
}

impl<T> ConsoleCommand<'_, T> {
    ///
    /// Returns Some(T) if the command was executed and arguments were valid.
    ///
    /// This method should only be called once.
    /// Consecutive calls will return None regardless if the command occurred.
    pub fn take(&mut self) -> Option<Result<T, clap::Error>> {
        mem::take(&mut self.command)
    }

    /// Print `[ok]` in the console.
    pub fn ok(&mut self) {
        self.console_line
            .write(PrintConsoleLine::new("[ok]".into()));
    }

    /// Print `[failed]` in the console.
    pub fn failed(&mut self) {
        self.console_line
            .write(PrintConsoleLine::new("[failed]".into()));
    }

    /// Print a reply in the console.
    ///
    /// See [`reply!`](crate::reply) for usage with the [`format!`] syntax.
    pub fn reply(&mut self, msg: impl Into<String>) {
        self.console_line.write(PrintConsoleLine::new(msg.into()));
    }

    /// Print a reply in the console followed by `[ok]`.
    ///
    /// See [`reply_ok!`](crate::reply_ok) for usage with the [`format!`] syntax.
    pub fn reply_ok(&mut self, msg: impl Into<String>) {
        self.console_line.write(PrintConsoleLine::new(msg.into()));
        self.ok();
    }

    /// Print a reply in the console followed by `[failed]`.
    ///
    /// See [`reply_failed!`](crate::reply_failed) for usage with the [`format!`] syntax.
    pub fn reply_failed(&mut self, msg: impl Into<String>) {
        self.console_line.write(PrintConsoleLine::new(msg.into()));
        self.failed();
    }
}

pub struct ConsoleCommandState<T> {
    #[allow(clippy::type_complexity)]
    message_reader: <ConsoleCommandEnteredReaderSystemParam as SystemParam>::State,
    console_line: <PrintConsoleLineWriterSystemParam as SystemParam>::State,
    marker: PhantomData<T>,
}

unsafe impl<T: Command> SystemParam for ConsoleCommand<'_, T> {
    type State = ConsoleCommandState<T>;
    type Item<'w, 's> = ConsoleCommand<'w, T>;

    fn init_state(world: &mut World) -> Self::State {
        let message_reader = ConsoleCommandEnteredReaderSystemParam::init_state(world);
        let console_line = PrintConsoleLineWriterSystemParam::init_state(world);
        ConsoleCommandState {
            message_reader,
            console_line,
            marker: PhantomData,
        }
    }

    fn init_access(
        _state: &Self::State,
        _system_meta: &mut SystemMeta,
        _component_access_set: &mut FilteredAccessSet,
        _world: &mut World,
    ) {
    }

    #[inline]
    unsafe fn get_param<'w, 's>(
        state: &'s mut Self::State,
        system_meta: &SystemMeta,
        world: UnsafeWorldCell<'w>,
        change_tick: Tick,
    ) -> Self::Item<'w, 's> {
        unsafe {
            let mut message_reader = ConsoleCommandEnteredReaderSystemParam::get_param(
                &mut state.message_reader,
                system_meta,
                world,
                change_tick,
            );
            let mut console_line = PrintConsoleLineWriterSystemParam::get_param(
                &mut state.console_line,
                system_meta,
                world,
                change_tick,
            );

            let command = message_reader.read().find_map(|command| {
                if T::name() == command.command_name {
                    let clap_command = T::command().no_binary_name(true);
                    // .color(clap::ColorChoice::Always);
                    let arg_matches = clap_command.try_get_matches_from(command.args.iter());

                    debug!(
                        "Trying to parse as `{}`. Result: {arg_matches:?}",
                        command.command_name
                    );

                    match arg_matches {
                        Ok(matches) => {
                            return Some(T::from_arg_matches(&matches));
                        }
                        Err(err) => {
                            console_line.write(PrintConsoleLine::new(err.to_string()));
                            return Some(Err(err));
                        }
                    }
                }
                None
            });

            ConsoleCommand {
                command,
                console_line,
            }
        }
    }
}
/// Parsed raw console command into `command` and `args`.
#[derive(Clone, Debug, Event, Message)]
pub struct ConsoleCommandEntered {
    /// the command definition
    pub command_name: String,
    /// Raw parsed arguments
    pub args: Vec<String>,
}

/// Events to print to the console.
#[derive(Clone, Debug, Eq, Event, PartialEq, Message)]
pub struct PrintConsoleLine {
    /// Console line
    pub line: String,
}

impl PrintConsoleLine {
    /// Creates a new console line to print.
    pub const fn new(line: String) -> Self {
        Self { line }
    }
}

/// Size value for console dimensions.
///
/// Supports absolute pixels (`Px`) and percentages (`Percent`) of the current viewport.
#[derive(Clone, Copy, Debug)]
pub enum ConsoleSizeValue {
    /// Absolute size in logical pixels.
    Px(f32),
    /// Relative size in percentage of the viewport (0-100).
    Percent(f32),
}

impl ConsoleSizeValue {
    /// Resolves the value into logical pixels for the provided viewport size.
    pub fn resolve(self, viewport_size: f32) -> f32 {
        match self {
            Self::Px(px) => px.max(0.0),
            Self::Percent(percent) => (viewport_size * (percent.max(0.0) / 100.0)).max(0.0),
        }
    }
}

impl From<f32> for ConsoleSizeValue {
    fn from(value: f32) -> Self {
        Self::Px(value)
    }
}

impl From<&str> for ConsoleSizeValue {
    fn from(value: &str) -> Self {
        let trimmed = value.trim();
        if let Some(number) = trimmed.strip_suffix('%') {
            if let Ok(parsed) = number.trim().parse::<f32>() {
                return Self::Percent(parsed);
            }
            return Self::Px(0.0);
        }

        let lower = trimmed.to_ascii_lowercase();
        if let Some(number) = lower.strip_suffix("px") {
            if let Ok(parsed) = number.trim().parse::<f32>() {
                return Self::Px(parsed);
            }
            return Self::Px(0.0);
        }

        if let Ok(parsed) = trimmed.parse::<f32>() {
            return Self::Px(parsed);
        }

        Self::Px(0.0)
    }
}

impl From<String> for ConsoleSizeValue {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

/// Console configuration
#[derive(Resource)]
pub struct ConsoleConfiguration {
    /// Registered keys for toggling the console
    pub keys: Vec<KeyCode>,
    /// Left position
    pub left_pos: f32,
    /// Top position
    pub top_pos: f32,
    /// Console height (`"400px"`, `400.0`, or `"50%"`)
    pub height: ConsoleSizeValue,
    /// Console width (`"800px"`, `800.0`, or `"100%"`)
    pub width: ConsoleSizeValue,
    /// Registered console commands
    pub commands: BTreeMap<&'static str, clap::Command>,
    /// Number of commands to store in history
    pub history_size: usize,
    /// Line prefix symbol
    pub symbol: String,
    /// allows window to be collpased
    pub collapsible: bool,
    /// Title name of console window
    pub title_name: String,
    /// allows window to be resizable
    pub resizable: bool,
    /// allows window to be movable
    pub moveable: bool,
    /// show the title bar or not
    pub show_title_bar: bool,
    /// Background color of console window frame
    pub background_color: Color32,
    /// Background color of the scrollback/log area
    pub input_text_box_background_color: Color32,
    /// Background color of the input field area
    pub input_background_color: Color32,
    /// Foreground (text) color
    pub text_color: Color32,
    /// Number of suggested commands to show
    pub num_suggestions: usize,
    /// Background color of the suggestions popup/menu
    pub suggestions_background_color: Color32,
    /// Background color of the currently selected suggestion row
    pub suggestions_selected_background_color: Color32,
    /// Blocks mouse from clicking through console
    pub block_mouse: bool,
    /// Blocks keyboard from interacting outside console when active
    pub block_keyboard: bool,
    /// Custom completion sequences,
    /// for example [vec!["custom", "foo"]], will complete `custom foo` when typing `custom`
    pub arg_completions: Vec<Vec<String>>,
    /// Height of the input field itself (in pixels)
    /// Affects how tall the text input box appears
    /// Default: 30.0
    pub input_field_height: f32,
    /// Font size for input text (in pixels)
    /// Default: 14.0
    pub input_font_size: f32,
}

#[derive(Resource, Default)]
pub struct ConsoleCache {
    /// Trie used for completions, autogenerated from registered console commands
    /// this probably should operate over references to save memory, but this is convenient for now
    pub(crate) commands_trie: Option<Trie<u8>>,
    pub(crate) predictions_hash_key: Option<u64>,
    pub(crate) predictions_cache: Vec<String>,
    pub(crate) prediction_matches_buffer: bool,
}

impl Default for ConsoleConfiguration {
    fn default() -> Self {
        Self {
            keys: vec![KeyCode::Backquote],
            left_pos: 200.0,
            top_pos: 100.0,
            height: ConsoleSizeValue::Px(400.0),
            width: ConsoleSizeValue::Px(800.0),
            commands: BTreeMap::new(),
            history_size: 20,
            symbol: "$ ".to_owned(),
            collapsible: false,
            title_name: "Console".to_string(),
            resizable: true,
            moveable: true,
            show_title_bar: true,
            background_color: Color32::from_black_alpha(102),
            input_text_box_background_color: Color32::from_black_alpha(230),
            input_background_color: Color32::from_black_alpha(180),
            text_color: Color32::LIGHT_GRAY,
            num_suggestions: 4,
            suggestions_background_color: Color32::from_black_alpha(220),
            suggestions_selected_background_color: Color32::from_black_alpha(128),
            block_mouse: false,
            block_keyboard: false,
            arg_completions: Default::default(),
            input_field_height: 30.0,
            input_font_size: 14.0,
        }
    }
}

impl Clone for ConsoleConfiguration {
    fn clone(&self) -> ConsoleConfiguration {
        ConsoleConfiguration {
            keys: self.keys.clone(),
            left_pos: self.left_pos,
            top_pos: self.top_pos,
            height: self.height,
            width: self.width,
            commands: self.commands.clone(),
            history_size: self.history_size,
            symbol: self.symbol.clone(),
            collapsible: false,
            title_name: "Console".to_string(),
            resizable: true,
            moveable: true,
            show_title_bar: true,
            background_color: Color32::from_black_alpha(102),
            input_text_box_background_color: Color32::from_black_alpha(230),
            input_background_color: Color32::from_black_alpha(180),
            text_color: Color32::LIGHT_GRAY,
            num_suggestions: 4,
            suggestions_background_color: self.suggestions_background_color,
            suggestions_selected_background_color: self.suggestions_selected_background_color,
            block_mouse: self.block_mouse,
            block_keyboard: self.block_keyboard,
            arg_completions: self.arg_completions.clone(),
            input_field_height: self.input_field_height,
            input_font_size: self.input_font_size,
        }
    }
}

/// Add a console commands to Bevy app.
pub trait AddConsoleCommand {
    /// Add a console command with a given system.
    ///
    /// This registers the console command so it will print with the built-in `help` console command.
    ///
    /// # Example
    ///
    /// ```
    /// # use bevy::prelude::*;
    /// # use bevy_console::{AddConsoleCommand, ConsoleCommand};
    /// # use clap::Parser;
    /// App::new()
    ///     .add_console_command::<LogCommand, _>(log_command);
    /// #
    /// # /// Prints given arguments to the console.
    /// # #[derive(Parser, ConsoleCommand)]
    /// # #[command(name = "log")]
    /// # struct LogCommand;
    /// #
    /// # fn log_command(mut log: ConsoleCommand<LogCommand>) {}
    /// ```
    fn add_console_command<T: Command, Params>(
        &mut self,
        system: impl IntoScheduleConfigs<ScheduleSystem, Params>,
    ) -> &mut Self;
}

impl AddConsoleCommand for App {
    fn add_console_command<T: Command, Params>(
        &mut self,
        system: impl IntoScheduleConfigs<ScheduleSystem, Params>,
    ) -> &mut Self {
        let sys = move |mut config: ResMut<ConsoleConfiguration>| {
            let command = T::command().no_binary_name(true);
            // .color(clap::ColorChoice::Always);
            let name = T::name();
            if config.commands.contains_key(name) {
                warn!(
                    "console command '{}' already registered and was overwritten",
                    name
                );
            }
            config.commands.insert(name, command);
        };

        self.add_systems(Startup, sys.in_set(ConsoleSet::Startup))
            .add_systems(Update, system.in_set(ConsoleSet::Commands))
    }
}

/// Console open state
#[derive(Default, Resource)]
pub struct ConsoleOpen {
    /// Console open
    pub open: bool,
}

#[derive(Resource)]
pub(crate) struct ConsoleState {
    pub(crate) buf: String,
    pub(crate) scrollback: Vec<String>,
    pub(crate) history: VecDeque<String>,
    pub(crate) history_index: usize,
    pub(crate) suggestion_index: Option<usize>,
}

impl Default for ConsoleState {
    fn default() -> Self {
        ConsoleState {
            buf: String::default(),
            scrollback: Vec::new(),
            history: VecDeque::from([String::new()]),
            history_index: 0,
            suggestion_index: None,
        }
    }
}

fn default_style(config: &ConsoleConfiguration) -> TextFormat {
    TextFormat::simple(FontId::monospace(config.input_font_size), config.text_color)
}

fn style_ansi_text(str: &str, config: &ConsoleConfiguration) -> LayoutJob {
    let mut layout_job = LayoutJob::default();
    for (str, overrides) in parse_ansi_styled_str(str).into_iter() {
        let mut current_style = default_style(config);

        for o in overrides {
            match o {
                TextFormattingOverride::Bold => current_style.font_id.size = 16f32, // no support for bold font families in egui TODO: when egui supports bold font families, use them here
                TextFormattingOverride::Dim => {
                    // no support for dim font families in egui TODO: when egui supports dim font families, use them here
                    current_style.color = current_style.color.gamma_multiply(0.5);
                }
                TextFormattingOverride::Italic => current_style.italics = true,
                TextFormattingOverride::Underline => {
                    current_style.underline = egui::Stroke::new(1., config.text_color)
                }
                TextFormattingOverride::Strikethrough => {
                    current_style.strikethrough = egui::Stroke::new(1., config.text_color)
                }
                TextFormattingOverride::Foreground(c) => current_style.color = c,
                TextFormattingOverride::Background(c) => current_style.background = c,
                _ => {}
            }
        }

        if !str.is_empty() {
            layout_job.append(str, 0f32, current_style.clone());
        }
    }
    layout_job
}

/// Recompute predictions for the console based on the current buffer content.
/// if the buffer does not change the predictions are not recomputed.
pub(crate) fn recompute_predictions(
    state: &mut ConsoleState,
    cache: &mut ConsoleCache,
    suggestion_count: usize,
) {
    if state.buf.is_empty() {
        cache.predictions_cache.clear();
        cache.predictions_hash_key = None;
        cache.prediction_matches_buffer = false;
        state.suggestion_index = None;
        return;
    }

    let hash = FixedState::with_seed(42).hash_one(&state.buf);

    let recompute = if let Some(predictions_hash_key) = cache.predictions_hash_key {
        predictions_hash_key != hash
    } else {
        true
    };

    if recompute {
        let words = Shlex::new(&state.buf).collect::<Vec<_>>();
        let query = words.join(" ");

        let suggestions = match &cache.commands_trie {
            Some(trie) if !query.is_empty() => trie
                .predictive_search(query)
                .take(suggestion_count)
                .collect(),
            _ => vec![],
        };
        cache.predictions_cache = suggestions
            .into_iter()
            .map(|s| String::from_utf8(s).unwrap_or_default())
            .collect();

        cache.predictions_hash_key = Some(hash);
        state.suggestion_index = None;
        cache.prediction_matches_buffer = false;

        if let Some(first) = cache.predictions_cache.first()
            && cache.predictions_cache.len() == 1
            && first == &state.buf
        {
            cache.prediction_matches_buffer = true
        }
    }
}

pub(crate) fn console_ui(
    mut egui_context: EguiContexts,
    config: Res<ConsoleConfiguration>,
    mut cache: ResMut<ConsoleCache>,
    mut keyboard_input_events: MessageReader<KeyboardInput>,
    mut state: ResMut<ConsoleState>,
    command_entered: MessageWriter<ConsoleCommandEntered>,
    mut console_open: ResMut<ConsoleOpen>,
) {
    let keyboard_input_events = keyboard_input_events.read().collect::<Vec<_>>();

    // If there is no egui context, return (can happen when exiting the app)
    let ctx = if let Ok(ctxt) = egui_context.ctx_mut() {
        ctxt
    } else {
        return;
    };

    let pressed = keyboard_input_events
        .iter()
        .any(|code| console_key_pressed(code, &config.keys));

    let mut open_status_changed = false;

    // Toggle console
    if pressed && (console_open.open || !ctx.wants_keyboard_input()) {
        console_open.open = !console_open.open;
        open_status_changed = true;
    }

    if !console_open.open {
        return;
    }

    // Recompute predictions if the buffer changed
    recompute_predictions(&mut state, &mut cache, config.num_suggestions);

    let viewport = ctx.content_rect();
    let resolved_width = config.width.resolve(viewport.width());
    let resolved_height = config.height.resolve(viewport.height());
    let console_window_id = Id::new("bevy_console_window");

    egui::Window::new(&config.title_name)
        .id(console_window_id)
        .order(egui::Order::Foreground)
        .collapsible(config.collapsible)
        .default_pos([config.left_pos, config.top_pos])
        .fixed_size([resolved_width, resolved_height])
        .resizable(config.resizable)
        .movable(config.moveable)
        .title_bar(config.show_title_bar)
        .frame(egui::Frame {
            fill: config.background_color,
            ..Default::default()
        })
        .show(ctx, |ui| {
            ui.style_mut().visuals.extreme_bg_color = config.input_background_color;
            ui.style_mut().visuals.override_text_color = Some(config.text_color);

            // ------------------------
            // Bottom panel: input area
            // ------------------------
            egui::TopBottomPanel::bottom("console_input_panel")
                .frame(egui::Frame {
                    fill: Color32::TRANSPARENT,
                    inner_margin: egui::Margin {
                        left: 4,
                        right: 4,
                        top: 5,
                        bottom: 5,
                    },
                    ..Default::default()
                })
                .show_inside(ui, |ui| {
                    // ui.separator();

                    // Ctrl+C clears input
                    if ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::C)) {
                        state.buf.clear();
                        return;
                    }

                    // Ctrl+L clears history
                    if ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::L)) {
                        state.scrollback.clear();
                        return;
                    }

                    let mut input_style = (**ui.style()).clone();
                    input_style.text_styles.insert(
                        egui::TextStyle::Monospace,
                        egui::FontId::new(config.input_font_size, egui::FontFamily::Monospace),
                    );
                    ui.set_style(input_style);

                    let text_edit = egui::TextEdit::singleline(&mut state.buf)
                        .desired_width(f32::INFINITY)
                        .lock_focus(true)
                        .font(egui::TextStyle::Monospace)
                        .frame(true)
                        .margin(egui::vec2(6.0, 6.0));

                    let text_edit_response = ui.add(text_edit);

                    // Handle enter
                    handle_enter(
                        &config,
                        &cache,
                        &mut state,
                        command_entered,
                        ui,
                        &text_edit_response,
                    );

                    // History navigation
                    if text_edit_response.has_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::ArrowUp))
                        && state.history.len() > 1
                        && state.history_index < state.history.len() - 1
                    {
                        if state.history_index == 0 && !state.buf.trim().is_empty() {
                            *state.history.get_mut(0).unwrap() = state.buf.clone();
                        }

                        state.history_index += 1;
                        state.buf = state.history[state.history_index].clone();
                        set_cursor_pos(ui.ctx(), text_edit_response.id, state.buf.len());
                    } else if text_edit_response.has_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::ArrowDown))
                        && state.history_index > 0
                    {
                        state.history_index -= 1;
                        state.buf = state.history[state.history_index].clone();
                        set_cursor_pos(ui.ctx(), text_edit_response.id, state.buf.len());
                    }

                    // Tab cycles suggestions
                    if ui.input(|i| i.key_pressed(egui::Key::Tab))
                        && !cache.predictions_cache.is_empty()
                    {
                        match &mut state.suggestion_index {
                            Some(index) => {
                                *index = (*index + 1) % cache.predictions_cache.len();
                            }
                            None => {
                                state.suggestion_index = Some(0);
                            }
                        }
                    }

                    // Focus input when console just opened
                    if open_status_changed {
                        ui.memory_mut(|m| m.request_focus(text_edit_response.id));
                    }

                    // Suggestions popup
                    if text_edit_response.has_focus()
                        && !state.buf.is_empty()
                        && !cache.prediction_matches_buffer
                    {
                        let suggestions_area = egui::Area::new(ui.auto_id_with("suggestions"))
                            .order(egui::Order::Foreground)
                            .fixed_pos(text_edit_response.rect.left_bottom() + egui::vec2(0.0, 8.0))
                            .movable(false);

                        suggestions_area.show(ui.ctx(), |ui| {
                            egui::Frame::popup(ui.style())
                                .fill(config.suggestions_background_color)
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                                    
                                    for (i, suggestion) in cache.predictions_cache.iter().enumerate() {
                                        let is_highlighted = Some(i) == state.suggestion_index;

                                        ui.horizontal(|ui| {
                                            if is_highlighted {
                                                let bg_rect = ui.available_rect_before_wrap();
                                                ui.painter().rect_filled(
                                                    bg_rect,
                                                    0.0,
                                                    config.suggestions_selected_background_color,
                                                );
                                            }

                                            let label = egui::Label::new(
                                                egui::RichText::new(suggestion)
                                                    .monospace()
                                                    .size(config.input_font_size)
                                                    .color(egui::Color32::WHITE)
                                            );

                                            let response = ui.add(label);

                                            if is_highlighted {
                                                ui.painter().line_segment(
                                                    [
                                                        response.rect.left_bottom(),
                                                        response.rect.right_bottom(),
                                                    ],
                                                    egui::Stroke::new(1.0, egui::Color32::WHITE),
                                                );
                                            }
                                        });
                                    }
                                });
                        });
                    }
                });

            // ------------------------
            // Central panel: scrollback
            // ------------------------
            egui::CentralPanel::default()
                .frame(egui::Frame {
                    fill: config.input_text_box_background_color,
                    ..Default::default()
                })
                .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &state.scrollback {
                            ui.label(style_ansi_text(line, &config));
                        }

                        // Scroll to bottom if console just opened
                        if console_open.is_changed() {
                            ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                        }
                    });
            });
        });

    ctx.move_to_top(egui::LayerId::new(egui::Order::Foreground, console_window_id));
}

fn handle_enter(
    config: &Res<'_, ConsoleConfiguration>,
    cache: &ResMut<'_, ConsoleCache>,
    state: &mut ResMut<'_, ConsoleState>,
    mut command_entered: MessageWriter<'_, ConsoleCommandEntered>,
    ui: &mut egui::Ui,
    text_edit_response: &egui::Response,
) {
    // Handle enter
    if text_edit_response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        // if we have a selected suggestion
        // replace the content of the buffer with it and set the cursor to the end
        if let Some(index) = state.suggestion_index
            && index < cache.predictions_cache.len()
            && !cache.prediction_matches_buffer
        {
            state.buf = cache.predictions_cache[index].clone();
            state.suggestion_index = None;
            set_cursor_pos(ui.ctx(), text_edit_response.id, state.buf.len());
            return;
        }

        if state.buf.trim().is_empty() {
            state.scrollback.push(String::new());
        } else {
            let msg = format!("{}{}", config.symbol, state.buf);
            state.scrollback.push(msg);
            let cmd_string = state.buf.clone();
            state.history.insert(1, cmd_string);
            if state.history.len() > config.history_size + 1 {
                state.history.pop_back();
            }
            state.history_index = 0;

            let mut args = Shlex::new(&state.buf).collect::<Vec<_>>();

            if !args.is_empty() {
                let command_name = args.remove(0);
                debug!("Command entered: `{command_name}`, with args: `{args:?}`");

                let command = config.commands.get(command_name.as_str());

                if command.is_some() {
                    command_entered.write(ConsoleCommandEntered { command_name, args });
                } else {
                    debug!(
                        "Command not recognized, recognized commands: `{:?}`",
                        config.commands.keys().collect::<Vec<_>>()
                    );

                    state.scrollback.push("error: Invalid command".into());
                }
            }

            state.buf.clear();
        }
    }
}

pub(crate) fn receive_console_line(
    mut console_state: ResMut<ConsoleState>,
    mut messages: MessageReader<PrintConsoleLine>,
) {
    for message in messages.read() {
        let message: &PrintConsoleLine = message;
        console_state.scrollback.push(message.line.clone());
    }
}

fn console_key_pressed(keyboard_input: &KeyboardInput, configured_keys: &[KeyCode]) -> bool {
    if !keyboard_input.state.is_pressed() {
        return false;
    }

    for configured_key in configured_keys {
        if configured_key == &keyboard_input.key_code {
            return true;
        }
    }

    false
}

fn set_cursor_pos(ctx: &Context, id: Id, pos: usize) {
    if let Some(mut state) = TextEdit::load_state(ctx, id) {
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(pos))));
        state.store(ctx, id);
    }
}

pub fn block_mouse_input(
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    config: Res<ConsoleConfiguration>,
    mut contexts: EguiContexts,
) {
    if !config.block_mouse {
        return;
    }

    let Ok(context) = contexts.ctx_mut() else {
        return;
    };

    if context.is_pointer_over_area() || context.wants_pointer_input() {
        mouse.reset_all();
    }
}

pub fn block_keyboard_input(
    mut keyboard_keycode: ResMut<ButtonInput<KeyCode>>,
    config: Res<ConsoleConfiguration>,
    mut contexts: EguiContexts,
) {
    if !config.block_keyboard {
        return;
    }

    let Ok(context) = contexts.ctx_mut() else {
        return;
    };

    if context.wants_keyboard_input() {
        keyboard_keycode.reset_all();
    }
}

#[cfg(test)]
mod tests {
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key, NativeKey, NativeKeyCode};

    use super::*;

    #[test]
    fn test_console_key_pressed_scan_code() {
        let input = KeyboardInput {
            key_code: KeyCode::Unidentified(NativeKeyCode::Xkb(41)),
            logical_key: Key::Unidentified(NativeKey::Xkb(41)),
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
            repeat: false,
            text: None,
        };

        let config = vec![KeyCode::Unidentified(NativeKeyCode::Xkb(41))];

        let result = console_key_pressed(&input, &config);
        assert!(result);
    }

    #[test]
    fn test_console_wrong_key_pressed_scan_code() {
        let input = KeyboardInput {
            key_code: KeyCode::Unidentified(NativeKeyCode::Xkb(42)),
            logical_key: Key::Unidentified(NativeKey::Xkb(42)),
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
            repeat: false,
            text: None,
        };

        let config = vec![KeyCode::Unidentified(NativeKeyCode::Xkb(41))];

        let result = console_key_pressed(&input, &config);
        assert!(!result);
    }

    #[test]
    fn test_console_key_pressed_key_code() {
        let input = KeyboardInput {
            key_code: KeyCode::Backquote,
            logical_key: Key::Character("`".into()),
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
            repeat: false,
            text: None,
        };

        let config = vec![KeyCode::Backquote];

        let result = console_key_pressed(&input, &config);
        assert!(result);
    }

    #[test]
    fn test_console_wrong_key_pressed_key_code() {
        let input = KeyboardInput {
            key_code: KeyCode::KeyA,
            logical_key: Key::Character("A".into()),
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
            repeat: false,
            text: None,
        };

        let config = vec![KeyCode::Backquote];

        let result = console_key_pressed(&input, &config);
        assert!(!result);
    }

    #[test]
    fn test_console_key_right_key_but_not_pressed() {
        let input = KeyboardInput {
            key_code: KeyCode::Backquote,
            logical_key: Key::Character("`".into()),
            state: ButtonState::Released,
            window: Entity::PLACEHOLDER,
            repeat: false,
            text: None,
        };

        let config = vec![KeyCode::Backquote];

        let result = console_key_pressed(&input, &config);
        assert!(!result);
    }
}

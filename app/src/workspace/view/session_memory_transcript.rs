use std::{fmt, fs, path::PathBuf};

use pathfinder_color::ColorU;
use serde_json::Value;
use warp_core::ui::theme::Fill as ThemeFill;
use warpui::elements::{
    Border, ClippedScrollStateHandle, ClippedScrollable, Container, CornerRadius,
    CrossAxisAlignment, Element, Fill as ElementFill, Flex, FormattedTextElement,
    MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement, Radius, ScrollbarWidth, Text,
};
use warpui::fonts::Weight;
use warpui::ui_components::button::Button;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext};

use crate::appearance::Appearance;
use crate::pane_group::focus_state::PaneFocusHandle;
use crate::pane_group::pane::{view, BackingView, PaneConfiguration, PaneEvent};
use crate::session_memory::types::SessionMemorySource;
use crate::ui_components::blended_colors;
use crate::workspace::view::session_memory_board::SessionMemoryBoardAction;

const MAX_MESSAGE_CHARS: usize = 20_000;
const ACTION_BUTTON_HEIGHT: f32 = 28.;

#[derive(Debug, Clone)]
pub struct SessionMemoryTranscriptPaneInput {
    pub record_id: String,
    pub title: String,
    pub source: SessionMemorySource,
    pub path: PathBuf,
}

pub struct SessionMemoryTranscriptView {
    input: SessionMemoryTranscriptPaneInput,
    load_result: TranscriptLoadResult,
    scroll_state: ClippedScrollStateHandle,
    pane_configuration: warpui::ModelHandle<PaneConfiguration>,
    focus_handle: Option<PaneFocusHandle>,
    resume_mouse_state: MouseStateHandle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMemoryTranscriptEvent {
    Action(SessionMemoryBoardAction),
    Pane(PaneEvent),
}

impl SessionMemoryTranscriptView {
    pub fn new(input: SessionMemoryTranscriptPaneInput, ctx: &mut ViewContext<Self>) -> Self {
        let load_result = load_transcript(&input.path);
        let title = format!("Transcript: {}", input.title);

        Self {
            input,
            load_result,
            scroll_state: ClippedScrollStateHandle::default(),
            pane_configuration: ctx.add_model(|_| PaneConfiguration::new(title)),
            focus_handle: None,
            resume_mouse_state: MouseStateHandle::default(),
        }
    }

    pub fn pane_configuration(&self) -> warpui::ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    fn render_text(
        text: impl Into<String>,
        size: f32,
        color: ColorU,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        Text::new(text.into(), appearance.ui_font_family(), size)
            .with_color(color)
            .finish()
    }

    fn render_monospace(
        text: impl Into<String>,
        size: f32,
        color: ColorU,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        Text::new(text.into(), appearance.monospace_font_family(), size)
            .with_color(color)
            .finish()
    }

    fn render_heading(
        text: impl Into<String>,
        size: f32,
        color: ColorU,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        FormattedTextElement::from_str(text.into(), appearance.ui_font_family(), size)
            .with_color(color)
            .with_weight(Weight::Bold)
            .finish()
    }

    fn render_badge(
        label: impl Into<String>,
        text_color: ColorU,
        background: ColorU,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        Container::new(
            Text::new_inline(label.into(), appearance.ui_font_family(), 11.)
                .with_color(text_color)
                .finish(),
        )
        .with_background(ThemeFill::Solid(background))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(999.)))
        .with_vertical_padding(3.)
        .with_horizontal_padding(8.)
        .finish()
    }

    fn render_header(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let (message_count, subtitle) = match &self.load_result {
            TranscriptLoadResult::Loaded(messages) => {
                (messages.len(), format!("{} messages", messages.len()))
            }
            TranscriptLoadResult::Error(error) => (0, error.clone()),
        };

        let title = if self.input.title.trim().is_empty() {
            "Transcript".to_owned()
        } else {
            self.input.title.clone()
        };

        let title_column = Flex::column()
            .with_spacing(8.)
            .with_child(Self::render_heading(
                title,
                18.,
                blended_colors::text_main(theme, blended_colors::neutral_1(theme)),
                appearance,
            ))
            .with_child(
                Flex::row()
                    .with_spacing(8.)
                    .with_child(Self::render_badge(
                        source_label(self.input.source),
                        blended_colors::text_sub(theme, blended_colors::neutral_2(theme)),
                        blended_colors::neutral_2(theme),
                        appearance,
                    ))
                    .with_child(Self::render_badge(
                        subtitle,
                        blended_colors::text_sub(theme, blended_colors::neutral_2(theme)),
                        blended_colors::neutral_2(theme),
                        appearance,
                    ))
                    .with_child(Self::render_badge(
                        format!("{} lines parsed", message_count),
                        blended_colors::text_sub(theme, blended_colors::neutral_2(theme)),
                        blended_colors::neutral_2(theme),
                        appearance,
                    ))
                    .finish(),
            )
            .with_child(Self::render_monospace(
                self.input.path.display().to_string(),
                11.,
                blended_colors::text_sub(theme, blended_colors::neutral_1(theme)),
                appearance,
            ))
            .finish();

        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(title_column)
            .with_child(self.render_resume_button(app))
            .finish()
    }

    fn render_resume_button(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let text_color = blended_colors::text_main(theme, blended_colors::neutral_2(theme));
        let base = UiComponentStyles::default()
            .set_height(ACTION_BUTTON_HEIGHT)
            .set_background(ThemeFill::Solid(blended_colors::neutral_2(theme)).into())
            .set_border_radius(CornerRadius::with_all(Radius::Pixels(7.)))
            .set_font_color(text_color)
            .set_font_size(12.)
            .set_font_family_id(appearance.ui_font_family())
            .set_padding(Coords::uniform(0.).left(10.).right(10.));
        let hover = base
            .clone()
            .set_background(ThemeFill::Solid(blended_colors::neutral_3(theme)).into());

        Button::new(
            self.resume_mouse_state.clone(),
            base,
            Some(hover.clone()),
            Some(hover),
            None,
        )
        .with_text_label("Resume chat".to_owned())
        .build()
        .on_click(|ctx, _, _| {
            ctx.dispatch_typed_action(SessionMemoryTranscriptUiAction::Resume);
        })
        .finish()
    }

    fn render_body(&self, app: &AppContext) -> Box<dyn Element> {
        match &self.load_result {
            TranscriptLoadResult::Loaded(messages) if messages.is_empty() => {
                self.render_empty_state("Transcript is empty.", app)
            }
            TranscriptLoadResult::Loaded(messages) => self.render_messages(messages, app),
            TranscriptLoadResult::Error(error) => self.render_empty_state(error, app),
        }
    }

    fn render_empty_state(&self, message: impl Into<String>, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        Container::new(Self::render_text(
            message,
            13.,
            blended_colors::text_sub(theme, blended_colors::neutral_1(theme)),
            appearance,
        ))
        .with_background(ThemeFill::Solid(blended_colors::neutral_1(theme)))
        .with_border(Border::all(1.).with_border_color(blended_colors::neutral_3(theme)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
        .with_uniform_padding(18.)
        .finish()
    }

    fn render_messages(
        &self,
        messages: &[TranscriptMessage],
        app: &AppContext,
    ) -> Box<dyn Element> {
        let mut column = Flex::column()
            .with_spacing(10.)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

        for message in messages {
            column.add_child(self.render_message(message, app));
        }

        column.finish()
    }

    fn render_message(&self, message: &TranscriptMessage, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let neutral_text = blended_colors::text_sub(theme, blended_colors::neutral_2(theme));
        let neutral_background = blended_colors::neutral_2(theme);
        let (accent, background) = role_colors(message.role, neutral_text, neutral_background);
        let content_color = blended_colors::text_main(theme, blended_colors::neutral_1(theme));

        let mut meta = Flex::row().with_spacing(8.).with_child(Self::render_badge(
            message.role.label(),
            accent,
            background,
            appearance,
        ));

        if let Some(timestamp) = &message.timestamp {
            meta.add_child(Self::render_badge(
                timestamp.clone(),
                blended_colors::text_sub(theme, blended_colors::neutral_2(theme)),
                blended_colors::neutral_2(theme),
                appearance,
            ));
        }

        let content = if message.role.is_tool_like() {
            Self::render_monospace(message.content.clone(), 12., content_color, appearance)
        } else {
            Self::render_text(message.content.clone(), 13., content_color, appearance)
        };

        Container::new(
            Flex::column()
                .with_spacing(8.)
                .with_child(meta.finish())
                .with_child(content)
                .finish(),
        )
        .with_background(ThemeFill::Solid(blended_colors::neutral_1(theme)))
        .with_border(Border::all(1.).with_border_color(blended_colors::neutral_3(theme)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
        .with_uniform_padding(12.)
        .finish()
    }
}

impl Entity for SessionMemoryTranscriptView {
    type Event = SessionMemoryTranscriptEvent;
}

impl View for SessionMemoryTranscriptView {
    fn ui_name() -> &'static str {
        "SessionMemoryTranscriptView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let scrollable = ClippedScrollable::vertical(
            self.scroll_state.clone(),
            self.render_body(app),
            ScrollbarWidth::Custom(4.),
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            ElementFill::None,
        )
        .with_overlayed_scrollbar()
        .finish();

        Container::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(14.)
                .with_child(self.render_header(app))
                .with_child(warpui::elements::Shrinkable::new(1., scrollable).finish())
                .finish(),
        )
        .with_background(ThemeFill::Solid(blended_colors::neutral_1(theme)))
        .with_uniform_padding(18.)
        .finish()
    }
}

impl BackingView for SessionMemoryTranscriptView {
    type PaneHeaderOverflowMenuAction = ();
    type CustomAction = ();
    type AssociatedData = ();

    fn handle_pane_header_overflow_menu_action(
        &mut self,
        _action: &Self::PaneHeaderOverflowMenuAction,
        _ctx: &mut ViewContext<Self>,
    ) {
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.emit(SessionMemoryTranscriptEvent::Pane(PaneEvent::Close));
    }

    fn focus_contents(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.focus_self();
    }

    fn render_header_content(
        &self,
        _ctx: &view::HeaderRenderContext<'_>,
        _app: &AppContext,
    ) -> view::HeaderContent {
        view::HeaderContent::simple("Transcript")
    }

    fn set_focus_handle(&mut self, focus_handle: PaneFocusHandle, _ctx: &mut ViewContext<Self>) {
        self.focus_handle = Some(focus_handle);
    }
}

impl TypedActionView for SessionMemoryTranscriptView {
    type Action = SessionMemoryTranscriptUiAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            SessionMemoryTranscriptUiAction::Resume => {
                ctx.emit(SessionMemoryTranscriptEvent::Action(
                    SessionMemoryBoardAction::Restore(self.input.record_id.clone()),
                ));
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMemoryTranscriptUiAction {
    Resume,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TranscriptLoadResult {
    Loaded(Vec<TranscriptMessage>),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptRole {
    User,
    Assistant,
    System,
    Tool,
    Other,
}

impl TranscriptRole {
    fn label(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Assistant => "Assistant",
            Self::System => "System",
            Self::Tool => "Tool",
            Self::Other => "Event",
        }
    }

    fn is_tool_like(self) -> bool {
        matches!(self, Self::Tool | Self::System | Self::Other)
    }

    fn is_chat_message(self) -> bool {
        matches!(self, Self::User | Self::Assistant)
    }
}

impl fmt::Display for TranscriptRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptMessage {
    pub role: TranscriptRole,
    pub content: String,
    pub timestamp: Option<String>,
}

fn load_transcript(path: &PathBuf) -> TranscriptLoadResult {
    match fs::read_to_string(path) {
        Ok(contents) => TranscriptLoadResult::Loaded(parse_transcript(&contents)),
        Err(error) => TranscriptLoadResult::Error(format!("Could not read transcript: {error}")),
    }
}

pub fn parse_transcript(contents: &str) -> Vec<TranscriptMessage> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }

            let value: Value = serde_json::from_str(line).ok()?;
            parse_transcript_line(&value)
        })
        .collect()
}

fn parse_transcript_line(value: &Value) -> Option<TranscriptMessage> {
    if value.get("type").and_then(Value::as_str) == Some("response_item") {
        return parse_codex_response_item(value);
    }

    if let Some(message) = value.get("message") {
        return parse_claude_message(value, message);
    }

    parse_generic_message(value)
}

fn parse_codex_response_item(value: &Value) -> Option<TranscriptMessage> {
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }

    let role = payload
        .get("role")
        .and_then(Value::as_str)
        .map(role_from_str)
        .unwrap_or_else(|| {
            payload
                .get("type")
                .and_then(Value::as_str)
                .map(role_from_str)
                .unwrap_or(TranscriptRole::Other)
        });
    if !role.is_chat_message() {
        return None;
    }

    let content = extract_chat_content(payload.get("content")?)?;
    if is_internal_chat_content(&content) {
        return None;
    }

    Some(TranscriptMessage {
        role,
        content: truncate_content(content),
        timestamp: timestamp(value),
    })
}

fn parse_claude_message(value: &Value, message: &Value) -> Option<TranscriptMessage> {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .map(role_from_str)
        .or_else(|| value.get("type").and_then(Value::as_str).map(role_from_str))
        .unwrap_or(TranscriptRole::Other);
    if !role.is_chat_message() {
        return None;
    }

    let content = extract_chat_content(message.get("content")?)?;
    if is_internal_chat_content(&content) {
        return None;
    }

    Some(TranscriptMessage {
        role,
        content: truncate_content(content),
        timestamp: timestamp(value),
    })
}

fn parse_generic_message(value: &Value) -> Option<TranscriptMessage> {
    let role = value
        .get("role")
        .and_then(Value::as_str)
        .or_else(|| value.get("type").and_then(Value::as_str))
        .map(role_from_str)
        .unwrap_or(TranscriptRole::Other);
    if !role.is_chat_message() {
        return None;
    }

    let content = value
        .get("content")
        .or_else(|| value.get("payload"))
        .and_then(extract_chat_content)?;
    if is_internal_chat_content(&content) {
        return None;
    }

    Some(TranscriptMessage {
        role,
        content: truncate_content(content),
        timestamp: timestamp(value),
    })
}

fn extract_chat_content(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => non_empty(text.clone()),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().filter_map(extract_chat_content_part).collect();
            non_empty(parts.join("\n\n"))
        }
        Value::Object(_) => {
            if let Some(text) = value.get("text").and_then(Value::as_str) {
                return non_empty(text.to_owned());
            }

            if let Some(content) = value.get("content").and_then(extract_chat_content) {
                return non_empty(content);
            }

            None
        }
        _ => None,
    }
}

fn extract_chat_content_part(value: &Value) -> Option<String> {
    match value.get("type").and_then(Value::as_str) {
        Some("text") | Some("input_text") | Some("output_text") => value
            .get("text")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .and_then(non_empty),
        _ => None,
    }
}

fn is_internal_chat_content(content: &str) -> bool {
    let trimmed = content.trim();
    trimmed.starts_with("<system-reminder>")
        || trimmed.starts_with("# AGENTS.md instructions")
        || trimmed.starts_with("<command-name>")
        || trimmed.starts_with("<command-message>")
        || trimmed.starts_with("<command-args>")
        || trimmed.starts_with("<local-command-stdout>")
        || trimmed.starts_with("<local-command-stderr>")
}

fn role_from_str(role: &str) -> TranscriptRole {
    match role {
        "user" | "input" => TranscriptRole::User,
        "assistant" | "model" | "message" => TranscriptRole::Assistant,
        "system" | "session_meta" => TranscriptRole::System,
        "tool" | "tool_use" | "tool_result" | "function_call" | "function_call_output" => {
            TranscriptRole::Tool
        }
        _ => TranscriptRole::Other,
    }
}

fn timestamp(value: &Value) -> Option<String> {
    value
        .get("timestamp")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn truncate_content(content: String) -> String {
    if content.chars().count() <= MAX_MESSAGE_CHARS {
        return content;
    }

    let mut truncated = content.chars().take(MAX_MESSAGE_CHARS).collect::<String>();
    truncated.push_str("\n\n... truncated in Session Memory transcript pane ...");
    truncated
}

fn non_empty(text: String) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn source_label(source: SessionMemorySource) -> &'static str {
    match source {
        SessionMemorySource::WarpTerminal => "Terminal",
        SessionMemorySource::ClaudeCode => "Claude Code",
        SessionMemorySource::Codex => "Codex",
    }
}

fn role_colors(
    role: TranscriptRole,
    neutral_text: ColorU,
    neutral_background: ColorU,
) -> (ColorU, ColorU) {
    match role {
        TranscriptRole::User => (neutral_text, neutral_background),
        TranscriptRole::Assistant => (
            ColorU::new(10, 104, 83, 255),
            ColorU::new(215, 245, 237, 255),
        ),
        TranscriptRole::System => (
            ColorU::new(84, 62, 137, 255),
            ColorU::new(232, 226, 250, 255),
        ),
        TranscriptRole::Tool => (
            ColorU::new(95, 68, 22, 255),
            ColorU::new(248, 236, 210, 255),
        ),
        TranscriptRole::Other => (neutral_text, neutral_background),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_text_message() {
        let transcript = r#"{"timestamp":"2026-07-08T10:00:00Z","type":"user","message":{"role":"user","content":"hello"}}"#;

        let messages = parse_transcript(transcript);

        assert_eq!(
            messages,
            vec![TranscriptMessage {
                role: TranscriptRole::User,
                content: "hello".to_owned(),
                timestamp: Some("2026-07-08T10:00:00Z".to_owned()),
            }]
        );
    }

    #[test]
    fn parses_only_text_from_claude_tool_message() {
        let transcript = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"checking"},{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}]}}"#;

        let messages = parse_transcript(transcript);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, TranscriptRole::Assistant);
        assert_eq!(messages[0].content, "checking");
    }

    #[test]
    fn parses_codex_response_item() {
        let transcript = r#"{"timestamp":"2026-07-08T10:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"resume this"}]}}"#;

        let messages = parse_transcript(transcript);

        assert_eq!(
            messages,
            vec![TranscriptMessage {
                role: TranscriptRole::User,
                content: "resume this".to_owned(),
                timestamp: Some("2026-07-08T10:00:00Z".to_owned()),
            }]
        );
    }

    #[test]
    fn skips_codex_non_message_response_items() {
        let transcript = r#"{"timestamp":"2026-07-08T10:00:00Z","type":"response_item","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1}}}}"#;

        assert!(parse_transcript(transcript).is_empty());
    }

    #[test]
    fn skips_internal_claude_system_reminders() {
        let transcript = r#"{"timestamp":"2026-07-08T10:00:00Z","type":"user","message":{"role":"user","content":"<system-reminder>\ninternal\n</system-reminder>"}}"#;

        assert!(parse_transcript(transcript).is_empty());
    }

    #[test]
    fn skips_codex_agents_md_bootstrap_context() {
        let transcript = concat!(
            r##"{"timestamp":"2026-07-08T10:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions\n\n<INSTRUCTIONS>\n# Codex Global Guidance\n</INSTRUCTIONS>"}]}}"##,
            "\n",
            r#"{"timestamp":"2026-07-08T10:01:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}"#,
        );

        let messages = parse_transcript(transcript);

        assert_eq!(
            messages,
            vec![TranscriptMessage {
                role: TranscriptRole::User,
                content: "hello".to_owned(),
                timestamp: Some("2026-07-08T10:01:00Z".to_owned()),
            }]
        );
    }
}

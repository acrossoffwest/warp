use std::path::PathBuf;

use pathfinder_color::ColorU;
use warp_core::ui::theme::Fill as ThemeFill;
use warpui::elements::{
    Border, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container, CornerRadius,
    CrossAxisAlignment, Element, Fill as ElementFill, Flex, FormattedTextElement,
    MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement, Radius, ScrollbarWidth, Text,
};
use warpui::fonts::{FamilyId, Weight};
use warpui::ui_components::button::Button;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext};

use crate::appearance::Appearance;
use crate::pane_group::focus_state::PaneFocusHandle;
use crate::pane_group::pane::{view, BackingView, PaneConfiguration, PaneEvent};
pub use crate::session_memory::types::{
    command_preview, AgentPermissionMode, SessionMemoryKind, SessionMemoryRecord,
    SessionMemorySource, SessionMemoryStatus,
};
use crate::ui_components::blended_colors;

const FILTER_BUTTON_HEIGHT: f32 = 30.;
const ACTION_BUTTON_HEIGHT: f32 = 28.;

fn text_button_style(
    font_family_id: FamilyId,
    font_size: f32,
    height: f32,
    border_radius: f32,
    padding: Coords,
    background: ColorU,
    text_color: ColorU,
) -> UiComponentStyles {
    UiComponentStyles::default()
        .set_height(height)
        .set_background(ThemeFill::Solid(background).into())
        .set_border_radius(CornerRadius::with_all(Radius::Pixels(border_radius)))
        .set_font_color(text_color)
        .set_font_size(font_size)
        .set_font_family_id(font_family_id)
        .set_padding(padding)
}

fn text_button_styles(
    font_family_id: FamilyId,
    font_size: f32,
    height: f32,
    border_radius: f32,
    padding: Coords,
    background: ColorU,
    hover_background: ColorU,
    clicked_background: ColorU,
    text_color: ColorU,
    hover_text_color: ColorU,
    clicked_text_color: ColorU,
) -> (UiComponentStyles, UiComponentStyles, UiComponentStyles) {
    (
        text_button_style(
            font_family_id,
            font_size,
            height,
            border_radius,
            padding,
            background,
            text_color,
        ),
        text_button_style(
            font_family_id,
            font_size,
            height,
            border_radius,
            padding,
            hover_background,
            hover_text_color,
        ),
        text_button_style(
            font_family_id,
            font_size,
            height,
            border_radius,
            padding,
            clicked_background,
            clicked_text_color,
        ),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMemoryBoardFilter {
    All,
    Interrupted,
    Terminal,
    ClaudeCode,
    Codex,
    Live,
}

impl SessionMemoryBoardFilter {
    pub const ALL: [Self; 6] = [
        Self::All,
        Self::Interrupted,
        Self::Terminal,
        Self::ClaudeCode,
        Self::Codex,
        Self::Live,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Interrupted => "Interrupted",
            Self::Terminal => "Terminal",
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::Live => "Live",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMemoryBoardAction {
    Restore(String),
    RestoreInSplit(String),
    CopyLastCommand(String),
    OpenTranscript(String),
    Delete(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMemoryBoardEvent {
    Action(SessionMemoryBoardAction),
    Pane(PaneEvent),
}

impl SessionMemorySource {
    fn label(self) -> &'static str {
        match self {
            Self::WarpTerminal => "Terminal",
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
        }
    }
}

impl SessionMemoryStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Blocked => "blocked",
            Self::Success => "success",
            Self::UserClosed => "closed",
            Self::Interrupted => "interrupted",
            Self::Stale => "stale",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMemoryBoardRow {
    pub id: String,
    pub source: SessionMemorySource,
    pub kind: SessionMemoryKind,
    pub status: SessionMemoryStatus,
    pub title: String,
    pub cwd: Option<PathBuf>,
    pub project: Option<String>,
    pub native_session_id: Option<String>,
    pub transcript_path: Option<PathBuf>,
    pub last_command: Option<String>,
    pub permission_mode: AgentPermissionMode,
    pub updated_at: i64,
}

impl SessionMemoryBoardRow {
    pub fn should_show_dangerous_badge(&self) -> bool {
        self.permission_mode == AgentPermissionMode::Dangerous
    }

    fn location_label(&self) -> String {
        self.cwd
            .as_ref()
            .map(|cwd| cwd.display().to_string())
            .or_else(|| self.project.clone())
            .unwrap_or_else(|| "No working directory captured".to_owned())
    }

    fn detail_label(&self) -> String {
        if let Some(last_command) = command_preview(self.last_command.as_deref()) {
            return format!("last: {last_command}");
        }

        if let Some(session_id) = &self.native_session_id {
            return format!("session: {session_id}");
        }

        if let Some(transcript_path) = &self.transcript_path {
            return format!("transcript: {}", transcript_path.display());
        }

        "No command or session id captured".to_owned()
    }

    fn is_terminal(&self) -> bool {
        self.kind == SessionMemoryKind::Terminal || self.source == SessionMemorySource::WarpTerminal
    }
}

impl From<&SessionMemoryRecord> for SessionMemoryBoardRow {
    fn from(record: &SessionMemoryRecord) -> Self {
        Self {
            id: record.id.clone(),
            source: record.source,
            kind: record.kind,
            status: record.status,
            title: record.title.clone(),
            cwd: record.cwd.clone(),
            project: record.project.clone(),
            native_session_id: record.native_session_id.clone(),
            transcript_path: record.transcript_path.clone(),
            last_command: record.last_command.clone(),
            permission_mode: record.permission_mode,
            updated_at: record.last_seen_at,
        }
    }
}

pub fn rows_from_records(records: &[SessionMemoryRecord]) -> Vec<SessionMemoryBoardRow> {
    records.iter().map(SessionMemoryBoardRow::from).collect()
}

pub fn filter_rows(
    rows: &[SessionMemoryBoardRow],
    filter: SessionMemoryBoardFilter,
    query: &str,
) -> Vec<SessionMemoryBoardRow> {
    let normalized_query = query.trim().to_lowercase();

    rows.iter()
        .filter(|row| filter_matches(row, filter))
        .filter(|row| query_matches(row, &normalized_query))
        .cloned()
        .collect()
}

fn filter_matches(row: &SessionMemoryBoardRow, filter: SessionMemoryBoardFilter) -> bool {
    match filter {
        SessionMemoryBoardFilter::All => true,
        SessionMemoryBoardFilter::Interrupted => row.status == SessionMemoryStatus::Interrupted,
        SessionMemoryBoardFilter::Terminal => row.source == SessionMemorySource::WarpTerminal,
        SessionMemoryBoardFilter::ClaudeCode => row.source == SessionMemorySource::ClaudeCode,
        SessionMemoryBoardFilter::Codex => row.source == SessionMemorySource::Codex,
        SessionMemoryBoardFilter::Live => row.status == SessionMemoryStatus::Live,
    }
}

fn query_matches(row: &SessionMemoryBoardRow, normalized_query: &str) -> bool {
    if normalized_query.is_empty() {
        return true;
    }

    let haystacks = [
        Some(row.source.label().to_owned()),
        Some(row.status.label().to_owned()),
        Some(row.title.clone()),
        row.cwd.as_ref().map(|cwd| cwd.display().to_string()),
        row.project.clone(),
        row.native_session_id.clone(),
        row.transcript_path
            .as_ref()
            .map(|transcript_path| transcript_path.display().to_string()),
        row.last_command.clone(),
    ];

    haystacks
        .into_iter()
        .flatten()
        .any(|text| text.to_lowercase().contains(normalized_query))
}

#[derive(Default)]
struct FilterMouseStateHandles {
    all: MouseStateHandle,
    interrupted: MouseStateHandle,
    terminal: MouseStateHandle,
    claude_code: MouseStateHandle,
    codex: MouseStateHandle,
    live: MouseStateHandle,
}

impl FilterMouseStateHandles {
    fn handle_for(&self, filter: SessionMemoryBoardFilter) -> MouseStateHandle {
        match filter {
            SessionMemoryBoardFilter::All => self.all.clone(),
            SessionMemoryBoardFilter::Interrupted => self.interrupted.clone(),
            SessionMemoryBoardFilter::Terminal => self.terminal.clone(),
            SessionMemoryBoardFilter::ClaudeCode => self.claude_code.clone(),
            SessionMemoryBoardFilter::Codex => self.codex.clone(),
            SessionMemoryBoardFilter::Live => self.live.clone(),
        }
    }
}

#[derive(Default)]
struct RowMouseStateHandles {
    restore: MouseStateHandle,
    restore_in_split: MouseStateHandle,
    copy_last_command: MouseStateHandle,
    open_transcript: MouseStateHandle,
    delete: MouseStateHandle,
}

pub struct SessionMemoryBoard {
    rows: Vec<SessionMemoryBoardRow>,
    active_filter: SessionMemoryBoardFilter,
    search_query: String,
    pane_configuration: warpui::ModelHandle<PaneConfiguration>,
    focus_handle: Option<PaneFocusHandle>,
    scroll_state: ClippedScrollStateHandle,
    filter_mouse_states: FilterMouseStateHandles,
    row_mouse_states: Vec<RowMouseStateHandles>,
}

impl SessionMemoryBoard {
    pub fn new(rows: Vec<SessionMemoryBoardRow>, ctx: &mut ViewContext<Self>) -> Self {
        let row_mouse_states = rows
            .iter()
            .map(|_| RowMouseStateHandles::default())
            .collect();

        Self {
            rows,
            active_filter: SessionMemoryBoardFilter::All,
            search_query: String::new(),
            pane_configuration: ctx.add_model(|_| PaneConfiguration::new("Session Memory")),
            focus_handle: None,
            scroll_state: Default::default(),
            filter_mouse_states: FilterMouseStateHandles::default(),
            row_mouse_states,
        }
    }

    pub fn pane_configuration(&self) -> warpui::ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    pub fn set_filter(&mut self, filter: SessionMemoryBoardFilter, ctx: &mut ViewContext<Self>) {
        if self.active_filter != filter {
            self.active_filter = filter;
            ctx.notify();
        }
    }

    pub fn set_rows(&mut self, rows: Vec<SessionMemoryBoardRow>, ctx: &mut ViewContext<Self>) {
        self.row_mouse_states = rows
            .iter()
            .map(|_| RowMouseStateHandles::default())
            .collect();
        self.rows = rows;
        ctx.notify();
    }

    pub fn remove_row(&mut self, id: &str, ctx: &mut ViewContext<Self>) {
        if remove_row_by_id(&mut self.rows, id) {
            self.row_mouse_states = self
                .rows
                .iter()
                .map(|_| RowMouseStateHandles::default())
                .collect();
            ctx.notify();
        }
    }

    pub fn visible_rows(&self) -> Vec<SessionMemoryBoardRow> {
        filter_rows(&self.rows, self.active_filter, &self.search_query)
    }

    fn has_interrupted_rows(&self) -> bool {
        self.rows
            .iter()
            .any(|row| row.status == SessionMemoryStatus::Interrupted)
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

    fn render_banner(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let interrupted_count = self
            .rows
            .iter()
            .filter(|row| row.status == SessionMemoryStatus::Interrupted)
            .count();
        let banner_title = ColorU::new(76, 45, 9, 255);
        let banner_body = ColorU::new(122, 77, 18, 255);

        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Flex::column()
                        .with_spacing(3.)
                        .with_child(Self::render_heading(
                            format!("{interrupted_count} interrupted sessions found"),
                            14.,
                            banner_title,
                            appearance,
                        ))
                        .with_child(Self::render_text(
                            "Commands were not run automatically. Choose a restore action when ready.",
                            12.,
                            banner_body,
                            appearance,
                        ))
                        .finish(),
                )
                .with_child(Self::render_badge(
                    "Startup recovery",
                    banner_body,
                    ColorU::new(255, 241, 214, 255),
                    appearance,
                ))
                .finish(),
        )
        .with_background(ThemeFill::Solid(ColorU::new(255, 248, 237, 255)))
        .with_border(Border::all(1.).with_border_color(ColorU::new(239, 209, 154, 255)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
        .with_uniform_padding(12.)
        .with_margin_bottom(14.)
        .finish()
    }

    fn render_filter_tabs(&self, app: &AppContext) -> Box<dyn Element> {
        let mut filters = Flex::row()
            .with_spacing(8.)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);

        for filter in SessionMemoryBoardFilter::ALL {
            filters.add_child(self.render_filter_button(filter, app));
        }

        filters.finish()
    }

    fn render_filter_button(
        &self,
        filter: SessionMemoryBoardFilter,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let is_active = self.active_filter == filter;
        let text_color = if is_active {
            blended_colors::text_main(theme, blended_colors::neutral_4(theme))
        } else {
            blended_colors::text_sub(theme, blended_colors::neutral_2(theme))
        };
        let (base_style, hover_style, clicked_style) = text_button_styles(
            appearance.ui_font_family(),
            12.,
            FILTER_BUTTON_HEIGHT,
            8.,
            Coords::uniform(0.).left(11.).right(11.),
            if is_active {
                blended_colors::neutral_4(theme)
            } else {
                blended_colors::neutral_2(theme)
            },
            blended_colors::neutral_3(theme),
            blended_colors::neutral_4(theme),
            text_color,
            blended_colors::text_main(theme, blended_colors::neutral_3(theme)),
            blended_colors::text_main(theme, blended_colors::neutral_4(theme)),
        );

        Button::new(
            self.filter_mouse_states.handle_for(filter),
            base_style,
            Some(hover_style),
            Some(clicked_style),
            None,
        )
        .with_text_label(filter.label().to_owned())
        .build()
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(SessionMemoryBoardUiAction::SetFilter(filter));
        })
        .finish()
    }

    fn render_rows(&self, app: &AppContext) -> Box<dyn Element> {
        let visible_rows = self.visible_rows();

        if visible_rows.is_empty() {
            return self.render_empty_state(app);
        }

        let mut rows = Flex::column()
            .with_spacing(10.)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

        for row in visible_rows {
            if let Some(row_index) = self
                .rows
                .iter()
                .position(|candidate| candidate.id == row.id)
            {
                if let Some(mouse_states) = self.row_mouse_states.get(row_index) {
                    rows.add_child(self.render_row(&row, mouse_states, app));
                }
            }
        }

        rows.finish()
    }

    fn render_empty_state(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        Container::new(Self::render_text(
            "No sessions match this filter.",
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

    fn render_row(
        &self,
        row: &SessionMemoryBoardRow,
        mouse_states: &RowMouseStateHandles,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        let mut badges = Flex::row()
            .with_spacing(6.)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Self::render_badge(
                row.source.label(),
                blended_colors::text_sub(theme, blended_colors::neutral_2(theme)),
                blended_colors::neutral_2(theme),
                appearance,
            ))
            .with_child(Self::render_badge(
                row.status.label(),
                blended_colors::text_sub(theme, blended_colors::neutral_2(theme)),
                blended_colors::neutral_2(theme),
                appearance,
            ));

        if row.should_show_dangerous_badge() {
            badges.add_child(Self::render_badge(
                "Dangerous",
                ColorU::new(141, 45, 9, 255),
                ColorU::new(255, 232, 223, 255),
                appearance,
            ));
        }

        Container::new(
            Flex::column()
                .with_spacing(10.)
                .with_child(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                        .with_cross_axis_alignment(CrossAxisAlignment::Start)
                        .with_child(
                            ConstrainedBox::new(
                                Flex::column()
                                    .with_spacing(5.)
                                    .with_child(badges.finish())
                                    .with_child(Self::render_heading(
                                        row.title.clone(),
                                        14.,
                                        blended_colors::text_main(
                                            theme,
                                            blended_colors::neutral_1(theme),
                                        ),
                                        appearance,
                                    ))
                                    .with_child(Self::render_text(
                                        row.location_label(),
                                        12.,
                                        blended_colors::text_sub(
                                            theme,
                                            blended_colors::neutral_1(theme),
                                        ),
                                        appearance,
                                    ))
                                    .with_child(Self::render_text(
                                        row.detail_label(),
                                        12.,
                                        blended_colors::text_sub(
                                            theme,
                                            blended_colors::neutral_1(theme),
                                        ),
                                        appearance,
                                    ))
                                    .finish(),
                            )
                            .with_width(700.)
                            .finish(),
                        )
                        .with_child(Self::render_text(
                            short_row_id(&row.id),
                            11.,
                            blended_colors::text_sub(theme, blended_colors::neutral_1(theme)),
                            appearance,
                        ))
                        .finish(),
                )
                .with_child(self.render_row_actions(row, mouse_states, app))
                .finish(),
        )
        .with_background(ThemeFill::Solid(blended_colors::neutral_1(theme)))
        .with_border(Border::all(1.).with_border_color(blended_colors::neutral_3(theme)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
        .with_uniform_padding(12.)
        .finish()
    }

    fn render_row_actions(
        &self,
        row: &SessionMemoryBoardRow,
        mouse_states: &RowMouseStateHandles,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let mut actions = Flex::row()
            .with_spacing(8.)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);

        for action in row_actions(row) {
            let mouse_state = match action.kind {
                RowActionKind::Restore => mouse_states.restore.clone(),
                RowActionKind::RestoreInSplit => mouse_states.restore_in_split.clone(),
                RowActionKind::CopyLastCommand => mouse_states.copy_last_command.clone(),
                RowActionKind::OpenTranscript => mouse_states.open_transcript.clone(),
                RowActionKind::Delete => mouse_states.delete.clone(),
            };

            actions.add_child(Self::render_action_button(
                action.label,
                action.to_board_action(&row.id),
                mouse_state,
                app,
            ));
        }

        actions.finish()
    }

    fn render_action_button(
        label: &'static str,
        action: SessionMemoryBoardAction,
        mouse_state: MouseStateHandle,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let is_delete = matches!(action, SessionMemoryBoardAction::Delete(_));
        let (background, hover_background, text_color) = if is_delete {
            (
                ColorU::new(255, 232, 223, 255),
                ColorU::new(255, 214, 198, 255),
                ColorU::new(141, 45, 9, 255),
            )
        } else {
            (
                blended_colors::neutral_2(theme),
                blended_colors::neutral_3(theme),
                blended_colors::text_main(theme, blended_colors::neutral_2(theme)),
            )
        };
        let (base_style, hover_style, clicked_style) = text_button_styles(
            appearance.ui_font_family(),
            12.,
            ACTION_BUTTON_HEIGHT,
            7.,
            Coords::uniform(0.).left(10.).right(10.),
            background,
            hover_background,
            hover_background,
            text_color,
            text_color,
            text_color,
        );

        Button::new(
            mouse_state,
            base_style,
            Some(hover_style),
            Some(clicked_style),
            None,
        )
        .with_text_label(label.to_owned())
        .build()
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(SessionMemoryBoardUiAction::Emit(action.clone()));
        })
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
}

impl Entity for SessionMemoryBoard {
    type Event = SessionMemoryBoardEvent;
}

impl View for SessionMemoryBoard {
    fn ui_name() -> &'static str {
        "SessionMemoryBoard"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        let mut header = Flex::column()
            .with_spacing(14.)
            .with_child(Self::render_heading(
                "Session Memory",
                20.,
                blended_colors::text_main(theme, blended_colors::neutral_1(theme)),
                appearance,
            ));

        if self.has_interrupted_rows() {
            header.add_child(self.render_banner(app));
        }

        header.add_child(self.render_filter_tabs(app));

        let scrollable = ClippedScrollable::vertical(
            self.scroll_state.clone(),
            self.render_rows(app),
            ScrollbarWidth::Custom(4.),
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            ElementFill::None,
        )
        .with_overlayed_scrollbar()
        .finish();

        Container::new(
            Flex::column()
                .with_spacing(14.)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(header.finish())
                .with_child(warpui::elements::Shrinkable::new(1., scrollable).finish())
                .finish(),
        )
        .with_background(ThemeFill::Solid(blended_colors::neutral_1(theme)))
        .with_uniform_padding(18.)
        .finish()
    }
}

impl BackingView for SessionMemoryBoard {
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
        ctx.emit(SessionMemoryBoardEvent::Pane(PaneEvent::Close));
    }

    fn focus_contents(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.focus_self();
    }

    fn render_header_content(
        &self,
        _ctx: &view::HeaderRenderContext<'_>,
        _app: &AppContext,
    ) -> view::HeaderContent {
        view::HeaderContent::simple("Session Memory")
    }

    fn set_focus_handle(&mut self, focus_handle: PaneFocusHandle, _ctx: &mut ViewContext<Self>) {
        self.focus_handle = Some(focus_handle);
    }
}

impl TypedActionView for SessionMemoryBoard {
    type Action = SessionMemoryBoardUiAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            SessionMemoryBoardUiAction::SetFilter(filter) => self.set_filter(*filter, ctx),
            SessionMemoryBoardUiAction::Emit(action) => {
                if let SessionMemoryBoardAction::Delete(id) = action {
                    self.remove_row(id, ctx);
                }

                ctx.emit(SessionMemoryBoardEvent::Action(action.clone()))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMemoryBoardUiAction {
    SetFilter(SessionMemoryBoardFilter),
    Emit(SessionMemoryBoardAction),
}

fn remove_row_by_id(rows: &mut Vec<SessionMemoryBoardRow>, id: &str) -> bool {
    let old_len = rows.len();
    rows.retain(|row| row.id != id);
    rows.len() != old_len
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowActionKind {
    Restore,
    RestoreInSplit,
    CopyLastCommand,
    OpenTranscript,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowAction {
    kind: RowActionKind,
    label: &'static str,
}

impl RowAction {
    fn to_board_action(self, row_id: &str) -> SessionMemoryBoardAction {
        match self.kind {
            RowActionKind::Restore => SessionMemoryBoardAction::Restore(row_id.to_owned()),
            RowActionKind::RestoreInSplit => {
                SessionMemoryBoardAction::RestoreInSplit(row_id.to_owned())
            }
            RowActionKind::CopyLastCommand => {
                SessionMemoryBoardAction::CopyLastCommand(row_id.to_owned())
            }
            RowActionKind::OpenTranscript => {
                SessionMemoryBoardAction::OpenTranscript(row_id.to_owned())
            }
            RowActionKind::Delete => SessionMemoryBoardAction::Delete(row_id.to_owned()),
        }
    }
}

fn row_actions(row: &SessionMemoryBoardRow) -> Vec<RowAction> {
    let mut actions = if row.is_terminal() {
        vec![
            RowAction {
                kind: RowActionKind::Restore,
                label: "Restore tab",
            },
            RowAction {
                kind: RowActionKind::CopyLastCommand,
                label: "Copy last command",
            },
        ]
    } else {
        vec![
            RowAction {
                kind: RowActionKind::Restore,
                label: "Resume",
            },
            RowAction {
                kind: RowActionKind::RestoreInSplit,
                label: "Resume in split",
            },
        ]
    };

    if row.transcript_path.is_some() {
        actions.push(RowAction {
            kind: RowActionKind::OpenTranscript,
            label: "Open transcript",
        });
    }

    actions.push(RowAction {
        kind: RowActionKind::Delete,
        label: "Delete",
    });

    actions
}

fn short_row_id(id: &str) -> String {
    const PREFIX_LEN: usize = 8;

    if id.chars().count() <= PREFIX_LEN {
        return id.to_owned();
    }

    let prefix: String = id.chars().take(PREFIX_LEN).collect();
    format!("{prefix}...")
}

#[cfg(test)]
#[path = "session_memory_board_tests.rs"]
mod tests;

use warpui::{AppContext, ModelHandle, SingletonEntity, View, ViewContext, ViewHandle};

use crate::{
    app_state::LeafContents,
    pane_group::pane::{
        view::PaneView, BackingView, DetachType, PaneConfiguration, PaneContent, PaneGroup, PaneId,
        ShareableLink, ShareableLinkError,
    },
    session_memory::model::SessionMemoryModel,
    workspace::view::session_memory_board::{
        rows_from_records, SessionMemoryBoard, SessionMemoryBoardEvent,
    },
};

pub struct SessionMemoryPane {
    view: ViewHandle<PaneView<SessionMemoryBoard>>,
    pane_configuration: ModelHandle<PaneConfiguration>,
}

impl SessionMemoryPane {
    pub fn new<V: View>(ctx: &mut ViewContext<V>) -> Self {
        let rows = rows_from_records(SessionMemoryModel::as_ref(ctx).records());
        let board = ctx.add_typed_action_view(|ctx| SessionMemoryBoard::new(rows, ctx));
        let pane_configuration = board.as_ref(ctx).pane_configuration();
        let view = ctx.add_typed_action_view(|ctx| {
            let pane_id = PaneId::from_session_memory_pane_ctx(ctx);
            PaneView::new(pane_id, board, (), pane_configuration.clone(), ctx)
        });

        Self {
            view,
            pane_configuration,
        }
    }

    pub fn board_view(&self, ctx: &AppContext) -> ViewHandle<SessionMemoryBoard> {
        self.view.as_ref(ctx).child(ctx)
    }
}

impl PaneContent for SessionMemoryPane {
    fn id(&self) -> PaneId {
        PaneId::from_session_memory_pane_view(&self.view)
    }

    fn attach(
        &self,
        _group: &PaneGroup,
        focus_handle: crate::pane_group::focus_state::PaneFocusHandle,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        self.view
            .update(ctx, |view, ctx| view.set_focus_handle(focus_handle, ctx));

        let pane_id = self.id();
        let board = self.board_view(ctx);
        ctx.subscribe_to_view(&board, move |pane_group, _, event, ctx| match event {
            SessionMemoryBoardEvent::Action(action) => {
                ctx.emit(crate::pane_group::Event::SessionMemoryBoardAction(
                    action.clone(),
                ));
            }
            SessionMemoryBoardEvent::Pane(pane_event) => {
                pane_group.handle_pane_event(pane_id, pane_event, ctx);
            }
        });
    }

    fn detach(
        &self,
        _group: &PaneGroup,
        _detach_type: DetachType,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        let board = self.board_view(ctx);
        ctx.unsubscribe_to_view(&board);
    }

    fn snapshot(&self, _app: &AppContext) -> LeafContents {
        LeafContents::SessionMemory
    }

    fn has_application_focus(&self, ctx: &mut ViewContext<PaneGroup>) -> bool {
        self.view.is_self_or_child_focused(ctx)
    }

    fn focus(&self, ctx: &mut ViewContext<PaneGroup>) {
        self.view
            .as_ref(ctx)
            .child(ctx)
            .update(ctx, BackingView::focus_contents);
    }

    fn shareable_link(
        &self,
        _ctx: &mut ViewContext<PaneGroup>,
    ) -> Result<ShareableLink, ShareableLinkError> {
        Ok(ShareableLink::Base)
    }

    fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    fn is_pane_being_dragged(&self, ctx: &AppContext) -> bool {
        self.view.as_ref(ctx).is_being_dragged()
    }
}

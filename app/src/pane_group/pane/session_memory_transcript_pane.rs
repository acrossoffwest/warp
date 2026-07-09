use warpui::{AppContext, ModelHandle, View, ViewContext, ViewHandle};

use crate::{
    app_state::LeafContents,
    pane_group::pane::{
        view::PaneView, BackingView, DetachType, PaneConfiguration, PaneContent, PaneGroup, PaneId,
        ShareableLink, ShareableLinkError,
    },
    workspace::view::session_memory_transcript::{
        SessionMemoryTranscriptEvent, SessionMemoryTranscriptPaneInput, SessionMemoryTranscriptView,
    },
};

pub struct SessionMemoryTranscriptPane {
    view: ViewHandle<PaneView<SessionMemoryTranscriptView>>,
    pane_configuration: ModelHandle<PaneConfiguration>,
}

impl SessionMemoryTranscriptPane {
    pub fn new<V: View>(input: SessionMemoryTranscriptPaneInput, ctx: &mut ViewContext<V>) -> Self {
        let transcript =
            ctx.add_typed_action_view(|ctx| SessionMemoryTranscriptView::new(input, ctx));
        let pane_configuration = transcript.as_ref(ctx).pane_configuration();
        let view = ctx.add_typed_action_view(|ctx| {
            let pane_id = PaneId::from_session_memory_transcript_pane_ctx(ctx);
            PaneView::new(pane_id, transcript, (), pane_configuration.clone(), ctx)
        });

        Self {
            view,
            pane_configuration,
        }
    }

    fn transcript_view(&self, ctx: &AppContext) -> ViewHandle<SessionMemoryTranscriptView> {
        self.view.as_ref(ctx).child(ctx)
    }
}

impl PaneContent for SessionMemoryTranscriptPane {
    fn id(&self) -> PaneId {
        PaneId::from_session_memory_transcript_pane_view(&self.view)
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
        let transcript = self.transcript_view(ctx);
        ctx.subscribe_to_view(&transcript, move |pane_group, _, event, ctx| match event {
            SessionMemoryTranscriptEvent::Action(action) => {
                ctx.emit(crate::pane_group::Event::SessionMemoryBoardAction(
                    action.clone(),
                ));
            }
            SessionMemoryTranscriptEvent::Pane(pane_event) => {
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
        let transcript = self.transcript_view(ctx);
        ctx.unsubscribe_to_view(&transcript);
    }

    fn snapshot(&self, _app: &AppContext) -> LeafContents {
        LeafContents::SessionMemoryTranscript
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

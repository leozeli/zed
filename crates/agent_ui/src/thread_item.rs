//! Center-pane item wrapper for an agent conversation.
//!
//! [`ConversationView`] is the surface the Agent Panel renders today. It is not
//! a workspace [`Item`], so it can only live inside the panel. This module wraps
//! it in a minimal `Item` implementation so a thread can also be opened as a
//! regular tab in the center pane, next to editors and terminals.
//!
//! The thread itself is still created through [`AgentPanel`], so it keeps its
//! thread metadata, sidebar entry, and notification behavior. What this module
//! adds is the mapping from a workspace item id to a thread id, which is all
//! that is needed to restore the tab on the next launch.

use crate::{
    Agent, AgentPanel, ConversationView, NewCenterThread,
    thread_metadata_store::{ThreadId, ThreadMetadataStore},
};
use anyhow::Result;
use db::{
    query,
    sqlez::{domain::Domain, statement::Statement, thread_safe_connection::ThreadSafeConnection},
    sqlez_macros::sql,
};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, SharedString,
    Task, WeakEntity, Window, prelude::*,
};
use project::{DisableAiSettings, Project};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::{Icon, IconName, Tooltip, prelude::*, utils::WithRemSize};
use workspace::{
    Item, SerializableItem, Workspace, WorkspaceId, delete_unloaded_items,
    item::{ItemEvent, TabTooltipContent},
};

/// A workspace item that hosts an agent conversation in a center pane.
pub struct ThreadItem {
    conversation_view: Entity<ConversationView>,
    workspace: WeakEntity<Workspace>,
    workspace_id: Option<WorkspaceId>,
    focus_handle: FocusHandle,
    needs_serialize: bool,
}

impl ThreadItem {
    pub fn new(
        conversation_view: Entity<ConversationView>,
        workspace: WeakEntity<Workspace>,
        workspace_id: Option<WorkspaceId>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            conversation_view,
            workspace,
            workspace_id,
            focus_handle: cx.focus_handle(),
            needs_serialize: true,
        }
    }

    /// Opens a new agent thread as a tab in the current pane.
    pub fn deploy(
        workspace: &mut Workspace,
        action: &NewCenterThread,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        if DisableAiSettings::get_global(cx).disable_ai {
            return;
        }
        let Some(panel) = workspace.panel::<AgentPanel>(cx) else {
            return;
        };

        let agent = match &action.agent {
            Some(agent_id) => Agent::from(agent_id.clone()),
            None => panel.read(cx).selected_agent(cx),
        };

        let conversation_view =
            panel.update(cx, |panel, cx| panel.create_center_thread(agent, window, cx));

        let workspace_id = workspace.database_id();
        let weak_workspace = workspace.weak_handle();
        let item = cx.new(|cx| Self::new(conversation_view, weak_workspace, workspace_id, cx));
        workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);
    }
}

impl Focusable for ThreadItem {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.conversation_view.read(cx).focus_handle(cx)
    }
}

impl EventEmitter<ItemEvent> for ThreadItem {}

impl Render for ThreadItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if DisableAiSettings::get_global(cx).disable_ai {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .track_focus(&self.focus_handle)
                .child(Label::new("AI is disabled").color(Color::Muted))
                .into_any_element();
        }

        // Mirror the Agent Panel's font sizing so a thread reads the same in a
        // center tab as it does in the panel. `WithRemSize` only implements
        // `Styled`, so focus tracking lives on an outer container.
        let theme_settings = ThemeSettings::get_global(cx);
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .child(
                WithRemSize::new(theme_settings.agent_ui_font_size(cx))
                    .size_full()
                    .font_family(theme_settings.agent_ui_font_family().clone())
                    .child(self.conversation_view.clone()),
            )
            .into_any_element()
    }
}

impl Item for ThreadItem {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, cx: &App) -> SharedString {
        self.conversation_view.read(cx).title(cx)
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::ZedAgent).color(Color::Muted))
    }

    fn tab_tooltip_content(&self, cx: &App) -> Option<TabTooltipContent> {
        let title = self.conversation_view.read(cx).title(cx);
        Some(TabTooltipContent::Custom(Box::new(Tooltip::element(
            move |_window, _cx| {
                v_flex()
                    .child(Label::new(title.clone()))
                    .child(
                        Label::new("Agent Thread")
                            .color(Color::Muted)
                            .size(LabelSize::Small),
                    )
                    .into_any_element()
            },
        ))))
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        Some("Agent Thread Opened")
    }

    fn include_in_nav_history() -> bool {
        false
    }

    fn can_split(&self) -> bool {
        true
    }

    fn clone_on_split(
        &self,
        workspace_id: Option<WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<Entity<Self>>>
    where
        Self: Sized,
    {
        // Splitting an agent thread starts a fresh conversation with the same
        // agent rather than mirroring the existing one: two views sharing one
        // thread id would fight over the same ACP session.
        let agent = self.conversation_view.read(cx).agent_key().clone();
        let workspace = self.workspace.clone();
        let panel = workspace
            .clone()
            .upgrade()
            .and_then(|workspace| workspace.read(cx).panel::<AgentPanel>(cx));
        cx.spawn_in(window, async move |_this, cx| {
            let panel = panel?;
            cx.update(|window, cx| {
                let conversation_view =
                    panel.update(cx, |panel, cx| panel.create_center_thread(agent, window, cx));
                cx.new(|cx| Self::new(conversation_view, workspace, workspace_id, cx))
            })
            .ok()
        })
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event);
    }
}

impl SerializableItem for ThreadItem {
    fn serialized_item_kind() -> &'static str {
        "AgentThread"
    }

    fn cleanup(
        workspace_id: WorkspaceId,
        alive_items: Vec<workspace::ItemId>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let db = CenterThreadDb::global(cx);
        delete_unloaded_items(alive_items, workspace_id, "center_agent_threads", &db, cx)
    }

    fn serialize(
        &mut self,
        _workspace: &mut Workspace,
        item_id: workspace::ItemId,
        _closing: bool,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<()>>> {
        if !self.needs_serialize {
            return None;
        }
        let workspace_id = self.workspace_id?;
        let thread_id = self.conversation_view.read(cx).thread_id;
        self.needs_serialize = false;

        let db = CenterThreadDb::global(cx);
        Some(cx.background_spawn(async move {
            db.save_thread_id(item_id, workspace_id, thread_id).await
        }))
    }

    fn should_serialize(&self, _: &Self::Event) -> bool {
        self.needs_serialize
    }

    fn deserialize(
        _project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        workspace_id: WorkspaceId,
        item_id: workspace::ItemId,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Entity<Self>>> {
        let db = CenterThreadDb::global(cx);
        window.spawn(cx, async move |cx| {
            let thread_id = db
                .thread_id(item_id, workspace_id)?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing center agent thread row"))?;

            let metadata = cx.update(|_, cx| {
                ThreadMetadataStore::try_global(cx)
                    .and_then(|store| store.read(cx).entry(thread_id).cloned())
            })?;
            let metadata = metadata
                .ok_or_else(|| anyhow::anyhow!("thread {thread_id:?} is no longer in the store"))?;
            let agent = Agent::from(metadata.agent_id.clone());
            let work_dirs = metadata.folder_paths().clone();
            let title = metadata.title();

            // The Agent Panel may not exist yet: panels are loaded concurrently
            // with the center pane during workspace restore. Wait for it rather
            // than silently dropping the tab.
            let panel = AgentPanel::wait_for_panel(&workspace, &mut *cx).await?;

            cx.update(|window, cx| {
                let conversation_view = panel.update(cx, |panel, cx| {
                    panel.resume_center_thread(
                        agent,
                        thread_id,
                        Some(work_dirs),
                        title,
                        window,
                        cx,
                    )
                });
                cx.new(|cx| Self::new(conversation_view, workspace, Some(workspace_id), cx))
            })
        })
    }
}

/// Persists which thread a center-pane item is showing.
struct CenterThreadDb(ThreadSafeConnection);

impl Domain for CenterThreadDb {
    const NAME: &str = stringify!(CenterThreadDb);

    const MIGRATIONS: &[&str] = &[
        sql!(
            CREATE TABLE center_agent_threads (
                workspace_id INTEGER,
                item_id INTEGER,
                thread_id BLOB,
                PRIMARY KEY (workspace_id, item_id),
                FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id)
                ON DELETE CASCADE
            ) STRICT;
        ),
    ];
}

db::static_connection!(CenterThreadDb, [workspace::WorkspaceDb]);

impl CenterThreadDb {
    async fn save_thread_id(
        &self,
        item_id: workspace::ItemId,
        workspace_id: WorkspaceId,
        thread_id: ThreadId,
    ) -> Result<()> {
        self.write(move |conn| {
            let sql = "INSERT INTO center_agent_threads (workspace_id, item_id, thread_id) \
                       VALUES (?1, ?2, ?3) \
                       ON CONFLICT (workspace_id, item_id) DO UPDATE SET thread_id = excluded.thread_id";
            let mut statement = Statement::prepare(conn, sql)?;
            let next_index = statement.bind(&workspace_id, 1)?;
            let next_index = statement.bind(&item_id, next_index)?;
            statement.bind(&thread_id, next_index)?;
            statement.exec()
        })
        .await
    }

    query! {
        fn thread_id(item_id: workspace::ItemId, workspace_id: WorkspaceId) -> Result<Option<ThreadId>> {
            SELECT thread_id
            FROM center_agent_threads
            WHERE item_id = ? AND workspace_id = ?
        }
    }
}

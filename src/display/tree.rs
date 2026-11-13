//! The sidebar's arrangement: accounts, folders and separators that the
//! person sorts by dragging. Folders nest. Every account and folder keeps its
//! own open or closed state, which the expanded sidebar and the rail share.
use super::*;
use serde::{Deserialize, Serialize};

/// How far a folder's contents are indented.
const INDENT: f32 = 18.;
/// Space between entries; drop lines are drawn in its middle.
const GAP: f32 = 4.;
/// A closed folder opens when something is held over it this long.
const SPRING: Duration = Duration::from_millis(650);
/// Opening, landing and drop-line animations.
const TREE_ANIMATION: Duration = Duration::from_millis(220);
/// A press that moved less than this is a click, not a drag.
const CLICK_SLOP: f64 = 5.;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(super) enum Node {
    Account {
        id: String,
    },
    Folder {
        id: String,
        name: String,
        #[serde(default)]
        children: Vec<Node>,
    },
    Separator {
        id: String,
    },
}
impl Node {
    pub(super) fn key(&self) -> &str {
        match self {
            Node::Account { id } | Node::Folder { id, .. } | Node::Separator { id } => id,
        }
    }
    fn children(&self) -> &[Node] {
        match self {
            Node::Folder { children, .. } => children,
            _ => &[],
        }
    }
    /// Whether this is `key` or holds it.
    fn contains(&self, key: &str) -> bool {
        self.key() == key || self.children().iter().any(|c| c.contains(key))
    }
    fn new_folder(name: &str) -> Self {
        Node::Folder {
            id: format!("group:{}", uuid::Uuid::new_v4()),
            name: name.into(),
            children: vec![],
        }
    }
    fn new_separator() -> Self {
        Node::Separator {
            id: format!("separator:{}", uuid::Uuid::new_v4()),
        }
    }
}

/// Where a dragged entry lands.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum DropAt {
    Before(String),
    After(String),
    /// At the end of a folder.
    Into(String),
}
impl DropAt {
    fn key(&self) -> &str {
        match self {
            DropAt::Before(k) | DropAt::After(k) | DropAt::Into(k) => k,
        }
    }
}

/// The indices leading to `key`.
fn path(nodes: &[Node], key: &str) -> Option<Vec<usize>> {
    for (i, n) in nodes.iter().enumerate() {
        if n.key() == key {
            return Some(vec![i]);
        }
        if let Some(mut rest) = path(n.children(), key) {
            rest.insert(0, i);
            return Some(rest);
        }
    }
    None
}
/// The list of entries at `path`: the root, or a folder's contents.
fn list_mut<'a>(nodes: &'a mut Vec<Node>, path: &[usize]) -> Option<&'a mut Vec<Node>> {
    let Some((first, rest)) = path.split_first() else {
        return Some(nodes);
    };
    match nodes.get_mut(*first)? {
        Node::Folder { children, .. } => list_mut(children, rest),
        _ => None,
    }
}
fn node_mut<'a>(nodes: &'a mut Vec<Node>, key: &str) -> Option<&'a mut Node> {
    let p = path(nodes, key)?;
    let (last, parent) = p.split_last()?;
    list_mut(nodes, parent)?.get_mut(*last)
}
pub(super) fn find<'a>(nodes: &'a [Node], key: &str) -> Option<&'a Node> {
    nodes.iter().find_map(|n| {
        if n.key() == key {
            Some(n)
        } else {
            find(n.children(), key)
        }
    })
}
fn take(nodes: &mut Vec<Node>, key: &str) -> Option<Node> {
    let p = path(nodes, key)?;
    let (last, parent) = p.split_last()?;
    Some(list_mut(nodes, parent)?.remove(*last))
}
fn insert(nodes: &mut Vec<Node>, at: &DropAt, node: Node) -> Result<(), Node> {
    match at {
        DropAt::Into(key) => match node_mut(nodes, key) {
            Some(Node::Folder { children, .. }) => {
                children.push(node);
                Ok(())
            }
            _ => Err(node),
        },
        DropAt::Before(key) | DropAt::After(key) => {
            let Some(p) = path(nodes, key) else {
                return Err(node);
            };
            let (last, parent) = p.split_last().expect("paths are never empty");
            let list = list_mut(nodes, parent).expect("a found path leads to a list");
            let at_end = matches!(at, DropAt::After(_)) as usize;
            list.insert(last + at_end, node);
            Ok(())
        }
    }
}
/// Moves `key` to `at`. A folder cannot go inside itself.
pub(super) fn move_node(nodes: &mut Vec<Node>, key: &str, at: &DropAt) -> bool {
    match find(nodes, key) {
        Some(moving) if !moving.contains(at.key()) => {}
        _ => return false,
    }
    let mut next = nodes.clone();
    let node = take(&mut next, key).expect("found above");
    if insert(&mut next, at, node).is_err() || next == *nodes {
        return false;
    }
    *nodes = next;
    true
}
/// Keeps the arrangement in step with the saved accounts (and aliases shown
/// as accounts): removed ones leave it, new ones are added at the end, and
/// duplicates are dropped.
pub(super) fn reconcile(nodes: &mut Vec<Node>, accounts: &[String]) -> bool {
    fn keep(nodes: &mut Vec<Node>, accounts: &[String], seen: &mut HashSet<String>) {
        nodes.retain_mut(|n| {
            if !seen.insert(n.key().to_owned()) {
                return false;
            }
            match n {
                Node::Account { id } => accounts.contains(id),
                Node::Folder { children, .. } => {
                    keep(children, accounts, seen);
                    true
                }
                Node::Separator { .. } => true,
            }
        });
    }
    let before = nodes.clone();
    let mut seen = HashSet::new();
    keep(nodes, accounts, &mut seen);
    for id in accounts {
        if !seen.contains(id) {
            nodes.push(Node::Account { id: id.clone() });
        }
    }
    *nodes != before
}
/// Accounts in the order the sidebar shows them.
pub(super) fn account_order(nodes: &[Node]) -> Vec<String> {
    let mut out = vec![];
    for n in nodes {
        match n {
            Node::Account { id } => out.push(id.clone()),
            Node::Folder { children, .. } => out.extend(account_order(children)),
            Node::Separator { .. } => {}
        }
    }
    out
}
/// The folder that holds `key` as its last entry, if any.
fn parent_if_last(nodes: &[Node], key: &str) -> Option<String> {
    let p = path(nodes, key)?;
    let (last, parent) = p.split_last()?;
    if parent.is_empty() {
        return None;
    }
    let mut folder = &nodes[parent[0]];
    for i in &parent[1..] {
        folder = &folder.children()[*i];
    }
    (*last + 1 == folder.children().len()).then(|| folder.key().to_owned())
}
/// Removes a folder, keeping its contents where it was.
fn unwrap_folder(nodes: &mut Vec<Node>, key: &str) -> bool {
    let Some(p) = path(nodes, key) else {
        return false;
    };
    let (last, parent) = p.split_last().expect("paths are never empty");
    let list = list_mut(nodes, parent).expect("a found path leads to a list");
    let Node::Folder { children, .. } = list.remove(*last) else {
        return false;
    };
    list.splice(*last..*last, children);
    true
}

/// What the person is doing with the arrangement right now.
#[derive(Default)]
pub(super) struct TreeUi {
    /// Where the entry being dragged would land.
    pub(super) drop_at: Option<DropAt>,
    /// Changes whenever `drop_at` does, keying its animation.
    serial: u64,
    pub(super) dragging: Option<String>,
    drag_start: Point<Pixels>,
    /// The folder whose name is being edited.
    pub(super) renaming: Option<String>,
    /// When each account or folder last opened or closed.
    toggled: HashMap<String, (u64, std::time::Instant)>,
    /// The entry that was just dropped or created, which flashes.
    landed: Option<(String, u64, std::time::Instant)>,
    anim: u64,
}

/// What travels with the pointer while an entry is dragged.
#[derive(Clone)]
pub(super) struct Dragged {
    pub(super) key: String,
}
pub(super) struct DragPreview {
    label: SharedString,
    icon: PreviewIcon,
    c: Colors,
    offset: Point<Pixels>,
}
#[derive(Clone)]
enum PreviewIcon {
    Avatar(Option<std::path::PathBuf>, String, usize),
    Icon(&'static str),
}
impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let c = self.c;
        let icon = match &self.icon {
            PreviewIcon::Avatar(Some(path), ..) => img(path.clone())
                .size(px(18.))
                .rounded_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            PreviewIcon::Avatar(None, initials, index) => {
                avatar(initials, 18., *index, c).into_any_element()
            }
            PreviewIcon::Icon(name) => icon_el(name, 15., c.accent).into_any_element(),
        };
        // The view is placed where the row was grabbed; the pill sits just
        // below and right of the pointer.
        div()
            .pl(px((f32::from(self.offset.x) - 12.).max(0.)))
            .pt(px((f32::from(self.offset.y) - 10.).max(0.)))
            .child(
                div()
                    .h(px(28.))
                    .max_w(px(240.))
                    .pl(px(6.))
                    .pr(px(12.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(8.))
                    .bg(rgb(c.raised))
                    .border_1()
                    .border_color(rgb(c.accent))
                    .shadow_lg()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(c.text))
                    .child(icon)
                    .child(div().min_w_0().truncate().child(self.label.clone()))
                    .with_animation(
                        "drag-preview",
                        Animation::new(Duration::from_millis(140)).with_easing(panes::ease_out),
                        |el, t| el.opacity(0.5 + 0.45 * t).top(px((1. - t) * 4.)),
                    ),
            )
    }
}

impl MailApp {
    pub(super) fn load_tree(store: &Store, accounts: &[String]) -> Vec<Node> {
        let mut tree: Vec<Node> = store
            .preference("sidebar_tree")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        reconcile(&mut tree, accounts);
        tree
    }
    pub(super) fn save_tree(&self) {
        if let Ok(json) = serde_json::to_string(&self.tree) {
            let _ = self.backend.store.set_preference("sidebar_tree", &json);
        }
    }
    /// Brings the arrangement up to date after accounts are added or removed.
    pub(super) fn sync_tree(&mut self) {
        if reconcile(
            &mut self.tree,
            &Self::tree_ids(&self.accounts, &self.forwarding),
        ) {
            self.save_tree();
        }
    }
    /// The first account the sidebar lists.
    pub(super) fn first_account(&self) -> Option<String> {
        account_order(&self.tree)
            .into_iter()
            .find(|id| !fwd::is_view(id))
    }
    pub(super) fn open(&self, key: &str) -> bool {
        !self.collapsed.contains(key)
    }
    /// Opens or closes an account or folder.
    pub(super) fn toggle_node(&mut self, key: String) {
        if !self.collapsed.remove(&key) {
            self.collapsed.insert(key.clone());
        }
        self.mark_toggled(key);
        self.save_collapsed();
    }
    fn mark_toggled(&mut self, key: String) {
        self.tree_ui.anim += 1;
        self.tree_ui
            .toggled
            .insert(key, (self.tree_ui.anim, std::time::Instant::now()));
    }
    fn mark_landed(&mut self, key: String) {
        self.tree_ui.anim += 1;
        self.tree_ui.landed = Some((key, self.tree_ui.anim, std::time::Instant::now()));
    }

    /// Adding: an account, a folder or a separator, inside `parent` or at the end.
    pub(super) fn create_entries(&self, parent: Option<String>) -> Vec<Entry> {
        vec![
            entry("Add Account…", "user", Action::Connect),
            Entry::Separator,
            entry(
                "New Folder",
                "folder-closed",
                Action::NewFolder(parent.clone()),
            ),
            entry("New Separator", "minus", Action::NewSeparator(parent)),
        ]
    }
    pub(super) fn folder_entries(&self, id: &str) -> Vec<Entry> {
        let id = id.to_owned();
        vec![
            entry("Rename…", "compose", Action::RenameFolder(id.clone())),
            entry(
                if self.open(&id) { "Collapse" } else { "Expand" },
                "chevron-down",
                Action::ToggleGroup(id.clone()),
            ),
            Entry::Separator,
            entry(
                "New Folder Inside",
                "folder-closed",
                Action::NewFolder(Some(id.clone())),
            ),
            entry(
                "New Separator Inside",
                "minus",
                Action::NewSeparator(Some(id.clone())),
            ),
            Entry::Separator,
            danger("Delete Folder", "trash", Action::DeleteFolder(id)),
        ]
    }
    fn separator_entries(&self, id: &str) -> Vec<Entry> {
        vec![danger(
            "Remove Separator",
            "trash",
            Action::RemoveSeparator(id.to_owned()),
        )]
    }
    /// Adds a folder or separator inside `parent` (opening it) or at the end.
    pub(super) fn add_node(
        &mut self,
        node: Node,
        parent: Option<String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = node.key().to_owned();
        let folder = matches!(node, Node::Folder { .. });
        let placed = match &parent {
            Some(p) => insert(&mut self.tree, &DropAt::Into(p.clone()), node).is_ok(),
            None => {
                self.tree.push(node);
                true
            }
        };
        if !placed {
            return;
        }
        if let Some(p) = parent
            && self.collapsed.remove(&p)
        {
            self.mark_toggled(p);
            self.save_collapsed();
        }
        self.mark_landed(key.clone());
        self.save_tree();
        if folder {
            self.start_rename(key, w, cx);
        }
        cx.notify();
    }
    pub(super) fn new_folder(
        &mut self,
        parent: Option<String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_node(Node::new_folder("New Folder"), parent, w, cx);
    }
    pub(super) fn new_separator(
        &mut self,
        parent: Option<String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_node(Node::new_separator(), parent, w, cx);
    }
    pub(super) fn delete_folder(&mut self, id: &str) {
        if unwrap_folder(&mut self.tree, id) {
            self.collapsed.remove(id);
            self.save_collapsed();
            self.save_tree();
        }
    }
    pub(super) fn remove_separator(&mut self, id: &str) {
        if matches!(find(&self.tree, id), Some(Node::Separator { .. })) {
            take(&mut self.tree, id);
            self.save_tree();
        }
    }
    /// Edits a folder's name, or an account's nickname, in place.
    pub(super) fn start_rename(&mut self, id: String, w: &mut Window, cx: &mut Context<Self>) {
        let (name, placeholder) = match find(&self.tree, &id) {
            Some(Node::Folder { name, .. }) => (name.clone(), "Folder name".to_owned()),
            Some(Node::Account { id }) => match self.accounts.iter().find(|a| &a.id == id) {
                Some(a) => (a.nickname.clone(), a.name.clone()),
                None => return,
            },
            _ => return,
        };
        self.tree_ui.renaming = Some(id);
        self.rename_input.update(cx, |s, cx| {
            s.set_placeholder(placeholder, w, cx);
            s.set_value(name, w, cx);
            s.focus(w, cx);
        });
        // Select the whole name once the field has focus, so typing replaces it.
        w.on_next_frame(|w, cx| w.dispatch_action(Box::new(gpui_component::input::SelectAll), cx));
        cx.notify();
    }
    /// Saves the edited name; an empty one keeps a folder's old name and
    /// clears an account's nickname.
    pub(super) fn commit_rename(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.tree_ui.renaming.take() else {
            return;
        };
        let value = self.rename_input.read(cx).value().trim().to_owned();
        if matches!(find(&self.tree, &id), Some(Node::Account { .. })) {
            self.set_nickname(&id, &value, cx);
        } else if !value.is_empty()
            && let Some(Node::Folder { name, .. }) = node_mut(&mut self.tree, &id)
        {
            *name = value;
            self.save_tree();
        }
        self.focus.focus(w);
        cx.notify();
    }
    /// Escape leaves the name as it was.
    pub(super) fn cancel_rename(&mut self, w: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.tree_ui.renaming.take().is_none() {
            return false;
        }
        self.focus.focus(w);
        cx.notify();
        true
    }

    /// The entry being dragged, while a drag is active.
    fn dragging(&self, cx: &App) -> Option<&str> {
        self.tree_ui
            .dragging
            .as_deref()
            .filter(|_| cx.has_active_drag())
    }
    fn drop_target(&self, cx: &App) -> Option<&DropAt> {
        self.tree_ui
            .drop_at
            .as_ref()
            .filter(|_| cx.has_active_drag())
    }
    fn set_drop(&mut self, at: Option<DropAt>, cx: &mut Context<Self>) {
        if self.tree_ui.drop_at == at {
            return;
        }
        self.tree_ui.drop_at = at;
        self.tree_ui.serial += 1;
        // Holding an entry over a closed folder opens it.
        if let Some(DropAt::Into(folder)) = &self.tree_ui.drop_at
            && self.collapsed.contains(folder)
        {
            let (folder, serial) = (folder.clone(), self.tree_ui.serial);
            cx.spawn(async move |this, cx| {
                Timer::after(SPRING).await;
                let _ = this.update(cx, |this, cx| {
                    if this.tree_ui.serial == serial && cx.has_active_drag() {
                        this.collapsed.remove(&folder);
                        this.mark_toggled(folder);
                        this.save_collapsed();
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }
    /// Where the dragged entry would land with the pointer `frac` of the way
    /// down `key`'s row, `out` indentation levels to the left of it.
    fn hover_target(&self, dragged: &str, key: &str, frac: f32, out: usize) -> Option<DropAt> {
        let node = find(&self.tree, key)?;
        let mut at = match node {
            Node::Folder { id, children, .. } => {
                if frac < 0.3 {
                    DropAt::Before(id.clone())
                } else if frac > 0.7 {
                    match children.first() {
                        Some(first) if self.open(id) => DropAt::Before(first.key().to_owned()),
                        _ => DropAt::After(id.clone()),
                    }
                } else {
                    DropAt::Into(id.clone())
                }
            }
            _ if frac < 0.5 => DropAt::Before(key.to_owned()),
            _ => DropAt::After(key.to_owned()),
        };
        // To the left of a folder's last entry, the drop leaves the folder.
        if let DropAt::After(k) = &at {
            let mut k = k.clone();
            for _ in 0..out {
                match parent_if_last(&self.tree, &k) {
                    Some(parent) => k = parent,
                    None => break,
                }
            }
            at = DropAt::After(k);
        }
        if find(&self.tree, dragged)?.contains(at.key()) {
            return None;
        }
        Some(at)
    }
    /// Tracks the pointer over a row while an entry is dragged.
    pub(super) fn on_hover_row(
        &mut self,
        key: &str,
        e: &DragMoveEvent<Dragged>,
        indented: bool,
        cx: &mut Context<Self>,
    ) {
        let (b, pos) = (e.bounds, e.event.position);
        if !b.contains(&pos) {
            return;
        }
        let dragged = e.drag(cx).key.clone();
        let frac = f32::from(pos.y - b.top()) / f32::from(b.size.height).max(1.);
        let out = if indented && pos.x < b.left() {
            (f32::from(b.left() - pos.x) / INDENT).ceil() as usize
        } else {
            0
        };
        let at = self.hover_target(&dragged, key, frac, out);
        self.set_drop(at, cx);
    }
    /// The list itself: leaving it clears the target, its empty end drops
    /// after the last entry, and its edges scroll.
    fn on_hover_list(
        &mut self,
        e: &DragMoveEvent<Dragged>,
        scroll: &ScrollHandle,
        cx: &mut Context<Self>,
    ) {
        let (b, pos) = (e.bounds, e.event.position);
        if !b.contains(&pos) {
            self.set_drop(None, cx);
            return;
        }
        let edge = 32.;
        let from_top = f32::from(pos.y - b.top());
        let from_bottom = f32::from(b.bottom() - pos.y);
        let step = if from_top < edge {
            (edge - from_top) / 2.
        } else if from_bottom < edge {
            -(edge - from_bottom) / 2.
        } else {
            0.
        };
        if step != 0. {
            let mut offset = scroll.offset();
            let max = f32::from(scroll.max_offset().height);
            offset.y = px((f32::from(offset.y) + step).clamp(-max, 0.));
            scroll.set_offset(offset);
            cx.notify();
        }
    }
    fn on_hover_end(&mut self, e: &DragMoveEvent<Dragged>, cx: &mut Context<Self>) {
        if !e.bounds.contains(&e.event.position) {
            return;
        }
        let dragged = e.drag(cx).key.clone();
        let at = self
            .tree
            .last()
            .filter(|last| last.key() != dragged)
            .map(|last| DropAt::After(last.key().to_owned()));
        self.set_drop(at, cx);
    }
    pub(super) fn on_dropped(&mut self, d: &Dragged, w: &mut Window, cx: &mut Context<Self>) {
        let at = self.tree_ui.drop_at.take();
        self.tree_ui.dragging = None;
        self.tree_ui.serial += 1;
        match at {
            Some(at) if move_node(&mut self.tree, &d.key, &at) => {
                self.save_tree();
                // A closed folder flashes to show where the entry went.
                let shown = match &at {
                    DropAt::Into(f) if !self.open(f) => f.clone(),
                    _ => d.key.clone(),
                };
                self.mark_landed(shown);
            }
            // A press that barely moved was meant as a click.
            _ if (w.mouse_position() - self.tree_ui.drag_start).magnitude() < CLICK_SLOP => {
                if matches!(
                    find(&self.tree, &d.key),
                    Some(Node::Account { .. } | Node::Folder { .. })
                ) {
                    self.toggle_node(d.key.clone());
                }
            }
            _ => {}
        }
        cx.notify();
    }
    /// Makes a row draggable as `key`.
    pub(super) fn draggable(
        &self,
        el: Stateful<Div>,
        key: &str,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        if self.tree_ui.renaming.as_deref() == Some(key) {
            return el;
        }
        let Some(node) = find(&self.tree, key) else {
            return el;
        };
        let (label, icon): (SharedString, PreviewIcon) = match node {
            Node::Account { id } if fwd::is_view(id) => (
                fwd::find_alias(&self.forwarding, id)
                    .map(|(_, a)| a.email.clone())
                    .unwrap_or_default()
                    .into(),
                PreviewIcon::Icon("alias"),
            ),
            Node::Account { id } => {
                let i = self.accounts.iter().position(|a| &a.id == id).unwrap_or(0);
                let a = &self.accounts[i.min(self.accounts.len().saturating_sub(1))];
                (
                    a.label().to_owned().into(),
                    PreviewIcon::Avatar(self.avatars.get(id).cloned(), initials(&a.name), i),
                )
            }
            Node::Folder { name, .. } => (name.clone().into(), PreviewIcon::Icon("folder-closed")),
            Node::Separator { .. } => ("Separator".into(), PreviewIcon::Icon("minus")),
        };
        let c = self.colors();
        let this = cx.weak_entity();
        el.on_drag(
            Dragged {
                key: key.to_owned(),
            },
            move |d: &Dragged, offset, w, cx| {
                let key = d.key.clone();
                let start = w.mouse_position();
                let _ = this.update(cx, |this, cx| {
                    this.tree_ui.dragging = Some(key);
                    this.tree_ui.drop_at = None;
                    this.tree_ui.drag_start = start;
                    this.menu = None;
                    cx.notify();
                });
                cx.new(|_| DragPreview {
                    label: label.clone(),
                    icon: icon.clone(),
                    c,
                    offset,
                })
            },
        )
    }

    /// The line where a dragged entry would go, drawn in the gap above or
    /// below `key`'s block.
    fn drop_line(&self, key: &str, inset: f32, cx: &App) -> Option<AnyElement> {
        let top = match self.drop_target(cx)? {
            DropAt::Before(k) if k == key => true,
            DropAt::After(k) if k == key => false,
            _ => return None,
        };
        let c = self.colors();
        let y = px(-(GAP / 2.) - 1.);
        Some(
            div()
                .absolute()
                .left(px(inset))
                .right(px(inset))
                .h(px(2.))
                .when(top, |d| d.top(y))
                .when(!top, |d| d.bottom(y))
                .rounded_full()
                .bg(rgb(c.accent))
                .child(
                    div()
                        .absolute()
                        .left(px(-4.))
                        .top(px(-2.))
                        .size(px(6.))
                        .rounded_full()
                        .border_2()
                        .border_color(rgb(c.accent))
                        .bg(rgb(c.bg)),
                )
                .with_animation(
                    ElementId::NamedInteger("drop-line".into(), self.tree_ui.serial),
                    Animation::new(Duration::from_millis(140)).with_easing(panes::ease_out),
                    |el, t| el.opacity(t),
                )
                .into_any_element(),
        )
    }
    /// A highlight over a folder that the dragged entry would go into, or
    /// over an entry that was just placed.
    fn block_overlay(&self, key: &str, cx: &App) -> Option<AnyElement> {
        let c = self.colors();
        if matches!(self.drop_target(cx), Some(DropAt::Into(k)) if k == key) {
            return Some(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(9.))
                    .bg(rgba((c.accent << 8) | 0x1a))
                    .border_1()
                    .border_color(rgb(c.accent))
                    .with_animation(
                        ElementId::NamedInteger("drop-into".into(), self.tree_ui.serial),
                        Animation::new(Duration::from_millis(160)).with_easing(panes::ease_out),
                        |el, t| el.opacity(t),
                    )
                    .into_any_element(),
            );
        }
        match &self.tree_ui.landed {
            Some((k, n, since)) if k == key && since.elapsed() < Duration::from_millis(900) => {
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(px(9.))
                        .bg(rgba((c.accent << 8) | 0x30))
                        .with_animation(
                            ElementId::NamedInteger(format!("landed-{k}").into(), *n),
                            Animation::new(Duration::from_millis(900)),
                            |el, t| el.opacity(1. - panes::ease_out(t)),
                        )
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }
    /// A block in the arrangement: an entry and, when open, its contents.
    pub(super) fn block(&self, key: &str, inset: f32, cx: &App) -> Div {
        let dragging = self.dragging(cx) == Some(key);
        div()
            .relative()
            .mt(px(GAP))
            .flex()
            .flex_col()
            .when(dragging, |d| d.opacity(0.35))
            .children(self.block_overlay(key, cx))
            .children(self.drop_line(key, inset, cx))
    }
    /// A chevron that turns as its account or folder opens and closes.
    pub(super) fn chevron(&self, key: &str, open: bool) -> AnyElement {
        let c = self.colors();
        let to = if open { 0.25 } else { 0. };
        let icon = svg()
            .path("icons/chevron-right.svg")
            .size(px(12.))
            .text_color(rgb(c.muted));
        let icon = match self.tree_ui.toggled.get(key) {
            Some((n, since)) if since.elapsed() < TREE_ANIMATION => icon
                .with_animation(
                    ElementId::NamedInteger(format!("chevron-{key}").into(), *n),
                    Animation::new(TREE_ANIMATION).with_easing(panes::ease_out),
                    move |s, t| {
                        let from = 0.25 - to;
                        s.with_transformation(Transformation::rotate(percentage(
                            from + (to - from) * t,
                        )))
                    },
                )
                .into_any_element(),
            _ => icon
                .with_transformation(Transformation::rotate(percentage(to)))
                .into_any_element(),
        };
        div()
            .size(px(12.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(icon)
            .into_any_element()
    }
    /// Contents slide in one after another just after their parent opens.
    pub(super) fn reveal<E: IntoElement + Styled + 'static>(
        &self,
        key: &str,
        el: E,
        index: usize,
    ) -> AnyElement {
        match self.tree_ui.toggled.get(key) {
            Some((n, since)) if self.open(key) && since.elapsed() < TREE_ANIMATION * 2 => {
                let delay = index.min(8) as f32 * 0.06;
                el.with_animation(
                    ElementId::NamedInteger(format!("reveal-{key}-{index}").into(), *n),
                    Animation::new(TREE_ANIMATION + Duration::from_millis(120)),
                    move |el, t| {
                        let t = panes::ease_out(((t - delay) / (1. - delay)).clamp(0., 1.));
                        el.opacity(t).top(px((1. - t) * -6.))
                    },
                )
                .into_any_element()
            }
            _ => el.into_any_element(),
        }
    }
    /// The unread count shown on a closed folder: its accounts' inboxes.
    fn folder_unread(&self, children: &[Node]) -> usize {
        account_order(children)
            .iter()
            .map(|id| self.unread_for(id, &Folder::Inbox.into()))
            .sum()
    }

    /// The expanded sidebar's entries.
    pub(super) fn sidebar_nodes(
        &self,
        nodes: &[Node],
        depth: usize,
        row: &Cell<usize>,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        nodes
            .iter()
            .filter_map(|n| match n {
                Node::Account { id } => self.account_block(id, depth, row, cx),
                Node::Folder { id, name, children } => {
                    Some(self.folder_block(id, name, children, depth, row, cx))
                }
                Node::Separator { id } => Some(self.separator_block(id, depth, cx)),
            })
            .collect()
    }
    pub(super) fn next_row(row: &Cell<usize>) -> usize {
        let i = row.get();
        row.set(i + 1);
        i
    }
    fn account_block(
        &self,
        id: &str,
        depth: usize,
        row: &Cell<usize>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if fwd::is_view(id) {
            return self.view_block(id, depth, row, true, cx);
        }
        let c = self.colors();
        let i = self.accounts.iter().position(|a| a.id == id)?;
        let a = &self.accounts[i];
        let open = self.open(id);
        let entries = self.account_entries(id, true);
        let renaming = self.tree_ui.renaming.as_deref() == Some(id);
        let inbox_unread = self.unread_for(id, &Folder::Inbox.into());
        let current = self.account.as_deref() == Some(id);
        let header = div()
            .id(SharedString::from(format!("account-{id}")))
            .debug_selector({
                let id = id.to_owned();
                move || format!("account-{id}")
            })
            .relative()
            .h(px(34.))
            .px_2()
            .rounded(px(8.))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            // The current account stays highlighted, open or closed.
            .when(current, |d| {
                d.bg(rgb(if open { c.surface } else { c.soft })).child(
                    div()
                        .absolute()
                        .left(px(-6.))
                        .top(px(9.))
                        .w(px(3.))
                        .h(px(16.))
                        .rounded_full()
                        .bg(rgb(c.accent)),
                )
            })
            .hover(move |s| s.bg(rgb(if current { c.soft } else { c.surface })))
            .child(self.chevron(id, open))
            .child(
                div()
                    .rounded_full()
                    .border_2()
                    .border_color(rgb(if current { c.accent } else { c.bg }))
                    .child(self.account_avatar(a, i, 20.)),
            )
            .child(if renaming {
                self.rename_field()
            } else {
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(if current { c.text } else { c.muted }))
                    .child(a.label().to_owned())
                    .into_any_element()
            })
            .when(
                self.account_issues
                    .get(id)
                    .is_some_and(|issues| !issues.is_empty()),
                |d| d.child(self.account_warning(id, cx)),
            )
            .when(!open && inbox_unread > 0, |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(c.muted))
                        .child(fmt_count(inbox_unread)),
                )
            })
            .tooltip({
                let email = a.email.clone();
                move |w, cx| Tooltip::new(email.clone()).build(w, cx)
            })
            .on_click(cx.listener({
                let id = id.to_owned();
                move |this, _, w, cx| {
                    if this.tree_ui.renaming.as_ref() == Some(&id) {
                        return;
                    }
                    this.handle(Action::ToggleAccount(id.clone()), w, cx)
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    this.open_menu(e.position, entries.clone(), cx)
                }),
            );
        let header = self.draggable(header, id, cx);
        let mut block = self
            .block(id, 0., cx)
            .id(SharedString::from(format!("block-{id}")))
            .on_drag_move(cx.listener({
                let id = id.to_owned();
                move |this, e: &DragMoveEvent<Dragged>, _, cx| {
                    this.on_hover_row(&id, e, depth > 0, cx)
                }
            }))
            .child(self.stagger(header, Self::next_row(row)));
        if !open {
            return Some(block.into_any_element());
        }
        let folders = self.folders.get(id).cloned().unwrap_or_default();
        let sub_open = !self.collapsed.contains(&format!("folders:{id}"));
        let mut index = 0;
        let mut add = |block: Stateful<Div>, el: Stateful<Div>| {
            index += 1;
            let el = self.reveal(
                id,
                div().child(self.stagger(el, Self::next_row(row))),
                index,
            );
            block.child(el)
        };
        for folder in Folder::ALL {
            let mut nav = self.nav_row(
                id,
                folder.into(),
                folder.label().into(),
                folder_icon(folder),
                0.,
                cx,
            );
            if folder == Folder::Inbox && !folders.is_empty() {
                let id = id.to_owned();
                nav = nav.child(
                    div()
                        .id(SharedString::from(format!("subfolders-{id}")))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .hover(move |s| s.bg(rgb(c.border)))
                        .child(icon_el(
                            if sub_open {
                                "chevron-down"
                            } else {
                                "chevron-right"
                            },
                            12.,
                            c.muted,
                        ))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, w, cx| {
                            cx.stop_propagation();
                            this.handle(Action::ToggleSubfolders(id.clone()), w, cx)
                        })),
                );
            }
            block = add(block, nav);
            if folder == Folder::Inbox && sub_open {
                for f in &folders {
                    let nav = self.nav_row(
                        id,
                        Mailbox::Remote(f.name.clone()),
                        f.label.clone(),
                        "folder",
                        14. + f.depth as f32 * 14.,
                        cx,
                    );
                    block = add(block, nav);
                }
            }
        }
        block = add(block, self.subscriptions_nav(id, cx));
        Some(block.into_any_element())
    }
    fn folder_block(
        &self,
        id: &str,
        name: &str,
        children: &[Node],
        depth: usize,
        row: &Cell<usize>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let open = self.open(id);
        let into = matches!(self.drop_target(cx), Some(DropAt::Into(k)) if k == id);
        let renaming = self.tree_ui.renaming.as_deref() == Some(id);
        let unread = if open {
            0
        } else {
            self.folder_unread(children)
        };
        let entries = self.folder_entries(id);
        let label = if renaming {
            self.rename_field()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(if into { c.text } else { c.muted }))
                .child(name.to_owned())
                .into_any_element()
        };
        let header = div()
            .id(SharedString::from(format!("folder-{id}")))
            .debug_selector({
                let id = id.to_owned();
                move || format!("folder-{id}")
            })
            .h(px(32.))
            .px_2()
            .rounded(px(8.))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(c.surface)))
            .child(self.chevron(id, open))
            .child(icon_el(
                if open || into {
                    "folder-open"
                } else {
                    "folder-closed"
                },
                16.,
                if into { c.accent } else { c.muted },
            ))
            .child(label)
            .when(unread > 0, |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(c.muted))
                        .child(fmt_count(unread)),
                )
            })
            .on_click(cx.listener({
                let id = id.to_owned();
                move |this, e: &ClickEvent, w, cx| {
                    if this.tree_ui.renaming.as_ref() == Some(&id) {
                        return;
                    }
                    if e.click_count() >= 2 {
                        this.start_rename(id.clone(), w, cx);
                    } else {
                        this.handle(Action::ToggleGroup(id.clone()), w, cx)
                    }
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    this.open_menu(e.position, entries.clone(), cx)
                }),
            )
            .on_drag_move(cx.listener({
                let id = id.to_owned();
                move |this, e: &DragMoveEvent<Dragged>, _, cx| {
                    this.on_hover_row(&id, e, depth > 0, cx)
                }
            }));
        let header = self.draggable(header, id, cx);
        let mut block = self
            .block(id, 0., cx)
            .child(self.stagger(header, Self::next_row(row)));
        if open {
            let mut contents = div()
                .ml(px(13.))
                .pl(px(INDENT - 13.))
                .border_l_1()
                .border_color(rgb(c.border))
                .flex()
                .flex_col();
            if children.is_empty() {
                contents = contents.child(
                    self.reveal(
                        id,
                        div()
                            .id(SharedString::from(format!("folder-empty-{id}")))
                            .mt(px(GAP))
                            .h(px(28.))
                            .px_2()
                            .flex()
                            .items_center()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child("Drag accounts here")
                            .on_drag_move(cx.listener({
                                let id = id.to_owned();
                                move |this, e: &DragMoveEvent<Dragged>, _, cx| {
                                    if !e.bounds.contains(&e.event.position) {
                                        return;
                                    }
                                    let dragged = e.drag(cx).key.clone();
                                    let at = find(&this.tree, &dragged)
                                        .filter(|n| !n.contains(&id))
                                        .map(|_| DropAt::Into(id.clone()));
                                    this.set_drop(at, cx);
                                }
                            })),
                        0,
                    ),
                );
            }
            for (i, el) in self
                .sidebar_nodes(children, depth + 1, row, cx)
                .into_iter()
                .enumerate()
            {
                contents = contents.child(self.reveal(id, div().child(el), i));
            }
            block = block.child(contents);
        }
        block.into_any_element()
    }
    /// The field that edits a folder's name or an account's nickname in place.
    fn rename_field(&self) -> AnyElement {
        let c = self.colors();
        div()
            .flex_1()
            .min_w_0()
            .h(px(24.))
            .rounded(px(5.))
            .border_1()
            .border_color(rgb(c.accent))
            .bg(rgb(c.bg))
            .flex()
            .items_center()
            .child(
                Input::new(&self.rename_input)
                    .appearance(false)
                    .text_size(px(13.)),
            )
            .into_any_element()
    }
    fn separator_block(&self, id: &str, depth: usize, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let entries = self.separator_entries(id);
        let group = SharedString::from(format!("separator-{id}"));
        let row = div()
            .id(group.clone())
            .debug_selector({
                let id = id.to_owned();
                move || format!("separator-{id}")
            })
            .group(group.clone())
            .h(px(12.))
            .px_2()
            .flex()
            .items_center()
            .cursor_grab()
            .child(
                div()
                    .flex_1()
                    .h(px(1.))
                    .bg(rgb(c.border))
                    .group_hover(group, move |s| s.bg(rgb(c.muted))),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    this.open_menu(e.position, entries.clone(), cx)
                }),
            )
            .on_drag_move(cx.listener({
                let id = id.to_owned();
                move |this, e: &DragMoveEvent<Dragged>, _, cx| {
                    this.on_hover_row(&id, e, depth > 0, cx)
                }
            }));
        self.block(id, 0., cx)
            .child(self.draggable(row, id, cx))
            .into_any_element()
    }
    /// The list's drag handling: dropping, leaving, and its empty end.
    pub(super) fn tree_list<E: InteractiveElement + 'static>(
        &self,
        list: E,
        scroll: &ScrollHandle,
        cx: &Context<Self>,
    ) -> E {
        let scroll = scroll.clone();
        list.on_drop(cx.listener(|this, d: &Dragged, w, cx| this.on_dropped(d, w, cx)))
            .on_drag_move(cx.listener(move |this, e: &DragMoveEvent<Dragged>, _, cx| {
                this.on_hover_list(e, &scroll, cx)
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    // Rows open their own menus first.
                    if this.menu.is_none() {
                        this.open_menu(e.position, this.create_entries(None), cx)
                    }
                }),
            )
    }
    /// Space after the last entry, which drops at the end.
    pub(super) fn tree_end(&self, id: &'static str, cx: &Context<Self>) -> Stateful<Div> {
        div().id(id).flex_1().min_h(px(36.)).on_drag_move(
            cx.listener(|this, e: &DragMoveEvent<Dragged>, _, cx| this.on_hover_end(e, cx)),
        )
    }

    /// The rail's entries: accounts as pictures, folders as folder buttons
    /// that open into a framed group, separators as short lines.
    pub(super) fn rail_nodes(
        &self,
        nodes: &[Node],
        row: &Cell<usize>,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let c = self.colors();
        let mut out = vec![];
        for n in nodes {
            let key = n.key().to_owned();
            let hover = cx.listener({
                let key = key.clone();
                move |this, e: &DragMoveEvent<Dragged>, _, cx| this.on_hover_row(&key, e, false, cx)
            });
            match n {
                Node::Account { id } if fwd::is_view(id) => {
                    if let Some(el) = self.rail_view(id, row, true, cx) {
                        out.push(el);
                    }
                }
                Node::Account { id } => {
                    let Some(i) = self.accounts.iter().position(|a| &a.id == id) else {
                        continue;
                    };
                    let a = &self.accounts[i];
                    let current = self.account.as_ref() == Some(id);
                    let open = self.open(id);
                    let entries = self.account_entries(id, false);
                    let unread = self.unread_for(id, &Folder::Inbox.into());
                    let button = self
                        .rail_button(
                            format!("rail-account-{id}"),
                            if a.nickname.is_empty() {
                                a.email.clone()
                            } else {
                                format!("{} · {}", a.nickname, a.email)
                            },
                            div()
                                .rounded_full()
                                .border_2()
                                .border_color(rgb(if current { c.accent } else { c.bg }))
                                .relative()
                                .child(self.account_avatar(a, i, 24.))
                                .when(!open && unread > 0, |d| {
                                    d.child(
                                        div()
                                            .absolute()
                                            .top(px(-2.))
                                            .right(px(-2.))
                                            .size(px(8.))
                                            .rounded_full()
                                            .border_1()
                                            .border_color(rgb(c.bg))
                                            .bg(rgb(c.accent)),
                                    )
                                })
                                .when(
                                    self.account_issues
                                        .get(id)
                                        .is_some_and(|issues| !issues.is_empty()),
                                    |d| {
                                        d.child(
                                            self.account_warning(id, cx)
                                                .absolute()
                                                .top(px(-7.))
                                                .right(px(-8.)),
                                        )
                                    },
                                )
                                .into_any_element(),
                            current && !open,
                            Action::ToggleAccount(id.clone()),
                            cx,
                        )
                        .debug_selector({
                            let id = id.clone();
                            move || format!("rail-account-{id}")
                        })
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                this.open_menu(e.position, entries.clone(), cx)
                            }),
                        );
                    let button = self.draggable(button, id, cx);
                    let mut block = self
                        .block(id, -4., cx)
                        .id(SharedString::from(format!("rail-block-{id}")))
                        .items_center()
                        .gap_1()
                        .on_drag_move(hover)
                        .child(self.stagger(button, Self::next_row(row)));
                    if open {
                        for (index, folder) in Folder::ALL.into_iter().enumerate() {
                            let mailbox: Mailbox = folder.into();
                            let active = current && self.mailbox() == mailbox;
                            let count = self.unread_for(id, &mailbox);
                            let entries = self.mailbox_entries(id, mailbox.clone());
                            let button = self
                                .rail_button(
                                    format!("rail-{id}-{}", folder.key()),
                                    if count > 0 {
                                        format!("{} · {} unread", folder.label(), fmt_count(count))
                                    } else {
                                        folder.label().into()
                                    },
                                    div()
                                        .relative()
                                        .child(icon_el(
                                            folder_icon(folder),
                                            17.,
                                            if active { c.accent } else { c.muted },
                                        ))
                                        .when(count > 0, |d| {
                                            d.child(
                                                div()
                                                    .absolute()
                                                    .top(px(-2.))
                                                    .right(px(-3.))
                                                    .size(px(7.))
                                                    .rounded_full()
                                                    .bg(rgb(c.accent)),
                                            )
                                        })
                                        .into_any_element(),
                                    active,
                                    Action::OpenMailbox(id.clone(), mailbox),
                                    cx,
                                )
                                .debug_selector({
                                    let selector = format!("rail-{id}-{}", folder.key());
                                    move || selector
                                })
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                        this.open_menu(e.position, entries.clone(), cx)
                                    }),
                                );
                            block = block.child(self.reveal(
                                id,
                                div().child(self.stagger(button, Self::next_row(row))),
                                index,
                            ));
                        }
                        let button = self.subscriptions_rail(id, cx);
                        block = block.child(self.reveal(
                            id,
                            div().child(self.stagger(button, Self::next_row(row))),
                            Folder::ALL.len(),
                        ));
                    }
                    out.push(block.into_any_element());
                }
                Node::Folder { id, name, children } => {
                    let open = self.open(id);
                    let into = matches!(self.drop_target(cx), Some(DropAt::Into(k)) if k == id);
                    let unread = if open {
                        0
                    } else {
                        self.folder_unread(children)
                    };
                    let entries = self.folder_entries(id);
                    let button = self
                        .rail_button(
                            format!("rail-folder-{id}"),
                            name.clone(),
                            div()
                                .relative()
                                .child(icon_el(
                                    if open || into {
                                        "folder-open"
                                    } else {
                                        "folder-closed"
                                    },
                                    18.,
                                    if into { c.accent } else { c.muted },
                                ))
                                .when(unread > 0, |d| {
                                    d.child(
                                        div()
                                            .absolute()
                                            .top(px(-2.))
                                            .right(px(-3.))
                                            .size(px(7.))
                                            .rounded_full()
                                            .bg(rgb(c.accent)),
                                    )
                                })
                                .into_any_element(),
                            false,
                            Action::ToggleGroup(id.clone()),
                            cx,
                        )
                        .debug_selector({
                            let id = id.clone();
                            move || format!("rail-folder-{id}")
                        })
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                this.open_menu(e.position, entries.clone(), cx)
                            }),
                        )
                        .on_drag_move(hover);
                    let button = self.draggable(button, id, cx);
                    let mut block = self
                        .block(id, -4., cx)
                        .items_center()
                        .child(self.stagger(button, Self::next_row(row)));
                    if open {
                        let mut frame = div()
                            .mt(px(2.))
                            .p(px(3.))
                            .rounded(px(12.))
                            .border_1()
                            .border_color(rgb(c.border))
                            .flex()
                            .flex_col()
                            .items_center();
                        if children.is_empty() {
                            frame = frame.child(div().size(px(20.)));
                        }
                        for (i, el) in self.rail_nodes(children, row, cx).into_iter().enumerate() {
                            frame = frame.child(self.reveal(id, div().child(el), i));
                        }
                        block = block.child(frame);
                    }
                    out.push(block.into_any_element());
                }
                Node::Separator { id } => {
                    let entries = self.separator_entries(id);
                    let line = div()
                        .id(SharedString::from(format!("rail-separator-{id}")))
                        .h(px(10.))
                        .w(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_grab()
                        .child(div().h(px(1.)).w(px(26.)).bg(rgb(c.border)))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                this.open_menu(e.position, entries.clone(), cx)
                            }),
                        )
                        .on_drag_move(hover);
                    out.push(
                        self.block(id, -4., cx)
                            .items_center()
                            .child(self.draggable(line, id, cx))
                            .into_any_element(),
                    );
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{DropAt, Node, account_order, move_node, parent_if_last, reconcile, unwrap_folder};

    fn acc(id: &str) -> Node {
        Node::Account { id: id.into() }
    }
    fn folder(id: &str, children: Vec<Node>) -> Node {
        Node::Folder {
            id: id.into(),
            name: id.into(),
            children,
        }
    }
    fn keys(nodes: &[Node]) -> Vec<String> {
        nodes
            .iter()
            .map(|n| match n {
                Node::Folder { id, children, .. } => format!("{id}[{}]", keys(children).join(",")),
                n => n.key().to_owned(),
            })
            .collect()
    }

    #[test]
    fn moves_between_lists_and_into_folders() {
        let mut t = vec![acc("a"), folder("f", vec![acc("b")]), acc("c")];
        assert!(move_node(&mut t, "c", &DropAt::Into("f".into())));
        assert_eq!(keys(&t), ["a", "f[b,c]"]);
        assert!(move_node(&mut t, "a", &DropAt::After("b".into())));
        assert_eq!(keys(&t), ["f[b,a,c]"]);
        assert!(move_node(&mut t, "c", &DropAt::Before("f".into())));
        assert_eq!(keys(&t), ["c", "f[b,a]"]);
        // Dropping where it already is changes nothing.
        assert!(!move_node(&mut t, "c", &DropAt::Before("f".into())));
    }

    #[test]
    fn folders_nest_but_never_inside_themselves() {
        let mut t = vec![
            folder("f", vec![folder("g", vec![acc("a")])]),
            folder("h", vec![]),
        ];
        assert!(!move_node(&mut t, "f", &DropAt::Into("g".into())));
        assert!(!move_node(&mut t, "f", &DropAt::After("a".into())));
        assert!(move_node(&mut t, "h", &DropAt::Into("g".into())));
        assert_eq!(keys(&t), ["f[g[a,h[]]]"]);
        assert_eq!(parent_if_last(&t, "h").as_deref(), Some("g"));
        assert_eq!(parent_if_last(&t, "a"), None);
    }

    #[test]
    fn reconcile_follows_accounts_and_deleting_a_folder_keeps_its_contents() {
        let mut t = vec![
            folder("f", vec![acc("gone"), acc("a"), acc("a")]),
            Node::new_separator(),
        ];
        assert!(reconcile(&mut t, &["a".into(), "b".into()]));
        assert_eq!(keys(&t)[0], "f[a]");
        assert_eq!(account_order(&t), ["a", "b"]);
        let mut t = vec![acc("x"), folder("f", vec![acc("a"), acc("b")]), acc("y")];
        assert!(unwrap_folder(&mut t, "f"));
        assert_eq!(keys(&t), ["x", "a", "b", "y"]);
    }

    #[test]
    fn saved_arrangement_round_trips() {
        let t = vec![folder(
            "f",
            vec![acc("a"), Node::Separator { id: "s".into() }],
        )];
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Vec<Node>>(&json).unwrap(), t);
    }
}

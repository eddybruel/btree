use std::{
    iter::FromIterator,
    mem, ops,
    ops::{Bound, Index, RangeBounds},
    sync::Arc,
};

#[cfg(test)]
const MAX_ITEM_COUNT: usize = 4;
#[cfg(not(test))]
const MAX_ITEM_COUNT: usize = 1024;

#[cfg(test)]
const MAX_NODE_COUNT: usize = 4;
#[cfg(not(test))]
const MAX_NODE_COUNT: usize = 8;

const MIN_ITEM_COUNT: usize = MAX_ITEM_COUNT / 2;

const MIN_NODE_COUNT: usize = MAX_NODE_COUNT / 2;

#[derive(Clone, Debug)]
pub struct BTree<T, Tag = ()> {
    height: usize,
    root: Node<T, Tag>,
}

impl<T, Tag> BTree<T, Tag> {
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.root.len()
    }

    pub fn tag(&self) -> &Tag {
        &self.root.tag
    }

    pub fn front(&self) -> Option<&'_ T> {
        if self.is_empty() {
            return None;
        }
        Some(Cursor::front(&self.root).get())
    }

    pub fn back(&self) -> Option<&'_ T> {
        if self.is_empty() {
            return None;
        }
        let mut cursor = Cursor::back(&self.root);
        cursor.move_previous();
        Some(cursor.get())
    }

    pub fn get(&self, index: usize) -> Option<&'_ T> {
        if index >= self.len() {
            return None;
        }
        Some(Cursor::at(&self.root, index).get())
    }

    pub fn iter(&self) -> Iter<'_, T, Tag> {
        Iter {
            count: self.len(),
            cursor: Cursor::front(&self.root),
        }
    }

    pub fn range<R: RangeBounds<usize>>(&self, range: R) -> Range<T, Tag> {
        let range = check_bounds(range, self.len());
        Range {
            count: range.len(),
            front: if range.start == self.len() {
                Cursor::back(&self.root)
            } else {
                Cursor::at(&self.root, range.start)
            },
            back: if range.end == self.len() {
                Cursor::back(&self.root)
            } else {
                Cursor::at(&self.root, range.end)
            },
        }
    }
}

impl<T, Tag: Monoid> BTree<T, Tag> {
    pub fn new() -> Self {
        Self {
            height: 0,
            root: Node::new(),
        }
    }
}

impl<T, Tag: Clone + Measure<T> + Monoid> BTree<T, Tag> {
    pub fn search<F>(&self, mut f: F) -> Option<usize>
    where
        F: FnMut(&Tag) -> bool,
    {
        let mut position = 0;
        let mut tag = Tag::empty();
        let mut node = &self.root;
        loop {
            match &node.kind {
                NodeKind::Leaf { items } => {
                    break Some(
                        position
                            + items.iter().position(|item| {
                                let new_tag = tag.clone().append(Tag::measure(item));
                                if f(&new_tag) {
                                    return true;
                                }
                                tag = new_tag;
                                false
                            })?,
                    );
                }
                NodeKind::Branch { nodes, .. } => {
                    node = nodes.iter().find(|node| {
                        let new_tag = tag.clone().append(node.tag.clone());
                        if f(&new_tag) {
                            return true;
                        }
                        position += node.len();
                        tag = new_tag;
                        false
                    })?;
                }
            }
        }
    }
}

impl<T: Clone, Tag: Clone + Measure<T> + Monoid> BTree<T, Tag> {
    pub fn splice<R: RangeBounds<usize>>(self, range: R, tree: Self) -> Self {
        let range = check_bounds(range, self.len());
        if range.start == range.end {
            if tree.is_empty() {
                return self;
            }
            let (tree_0, tree_1) = self.split(range.start);
            return tree_0.concat(tree).concat(tree_1);
        }
        if tree.is_empty() {
            return self
                .clone()
                .truncate_after(range.start)
                .concat(self.truncate_before(range.end));
        }
        self.clone()
            .truncate_after(range.start)
            .concat(tree)
            .concat(self.truncate_before(range.end))
    }

    pub fn insert(self, index: usize, item: T) -> Self {
        let (tree_0, tree_1) = self.split(index);
        tree_0.concat(tree_1.push_front(item))
    }

    pub fn remove(self, index: usize) -> Self {
        let (tree_0, tree_1) = self.split(index);
        tree_0.concat(tree_1.pop_front())
    }

    pub fn split(self, index: usize) -> (Self, Self) {
        assert!(index <= self.len());
        if index == 0 {
            return (Self::new(), self);
        }
        if index == self.len() {
            return (self, Self::new());
        }
        self.split_recursive(index)
    }

    fn split_recursive(self, index: usize) -> (Self, Self) {
        if self.height == 0 {
            let mut items = self.root.into_items();
            let other_items = Arc::make_mut(&mut items).drain(index..).collect::<Vec<_>>();
            return (
                Self {
                    height: 0,
                    root: Node::from_items(items),
                },
                Self {
                    height: 0,
                    root: Node::from_items(Arc::new(other_items)),
                },
            );
        }
        let mut nodes = self.root.into_nodes();
        let (index_0, index_1) = search(&nodes, index);
        let nodes_mut = Arc::make_mut(&mut nodes);
        let other_nodes = nodes_mut.drain(index_0 + 1..).collect::<Vec<_>>();
        let (tree_0, tree_1) = BTree {
            height: self.height - 1,
            root: nodes_mut.pop().unwrap(),
        }
        .split(index_1);
        (
            if nodes.is_empty() {
                tree_0
            } else {
                Self {
                    height: self.height,
                    root: Node::from_nodes(nodes),
                }
                .concat(tree_0)
            },
            if other_nodes.is_empty() {
                tree_1
            } else {
                tree_1.concat(Self {
                    height: self.height,
                    root: Node::from_nodes(Arc::new(other_nodes)),
                })
            },
        )
    }

    pub fn truncate_before(self, index: usize) -> Self {
        assert!(index <= self.len());
        if index == 0 {
            return self;
        }
        if index == self.len() {
            return Self::new();
        }
        self.truncate_before_recursive(index)
    }

    fn truncate_before_recursive(self, index: usize) -> Self {
        if self.height == 0 {
            let mut items = self.root.into_items();
            Arc::make_mut(&mut items).drain(..index);
            return Self {
                height: 0,
                root: Node::from_items(items),
            };
        }
        let mut nodes = self.root.into_nodes();
        let (index_0, index_1) = search(&nodes, index);
        let nodes_mut = Arc::make_mut(&mut nodes);
        nodes_mut.drain(..index_0);
        let tree = BTree {
            height: self.height - 1,
            root: nodes_mut.remove(0),
        }
        .truncate_before(index_1);
        if nodes.is_empty() {
            tree
        } else {
            tree.concat(Self {
                height: self.height,
                root: Node::from_nodes(nodes),
            })
        }
    }

    pub fn truncate_after(self, index: usize) -> Self {
        assert!(index <= self.len());
        if index == 0 {
            return Self::new();
        }
        if index == self.len() {
            return self;
        }
        self.truncate_after_recursive(index)
    }

    fn truncate_after_recursive(self, index: usize) -> Self {
        if self.height == 0 {
            let mut items = self.root.into_items();
            Arc::make_mut(&mut items).drain(index..);
            return Self {
                height: 0,
                root: Node::from_items(items),
            };
        }
        let mut nodes = self.root.into_nodes();
        let (index_0, index_1) = search(&nodes, index);
        let nodes_mut = Arc::make_mut(&mut nodes);
        nodes_mut.drain(index_0 + 1..);
        let tree = BTree {
            height: self.height - 1,
            root: nodes_mut.pop().unwrap(),
        }
        .truncate_after(index_1);
        if nodes.is_empty() {
            tree
        } else {
            Self {
                height: self.height,
                root: Node::from_nodes(nodes),
            }
            .concat(tree)
        }
    }

    pub fn pop_front(self) -> Self {
        if self.is_empty() {
            return self;
        }
        self.truncate_before(1)
    }

    pub fn pop_back(self) -> Self {
        if self.is_empty() {
            return self;
        }
        let len = self.len();
        self.truncate_after(len - 1)
    }

    pub fn concat(self: Self, other: Self) -> Self {
        use std::cmp::Ordering;

        match self.height.cmp(&other.height) {
            Ordering::Less => {
                let mut other_nodes = other.root.into_nodes();
                if self.height == other.height - 1 && self.root.is_at_least_half_full() {
                    return Self::prepend_node(other.height, self.root, other_nodes);
                }
                let tree = Self::concat(
                    self,
                    Self {
                        height: other.height - 1,
                        root: Arc::make_mut(&mut other_nodes).remove(0),
                    },
                );
                if tree.height == other.height - 1 {
                    Self::prepend_node(other.height, tree.root, other_nodes)
                } else {
                    Self::merge_nodes(other.height, tree.root.into_nodes(), other_nodes)
                }
            }
            Ordering::Equal => {
                if self.height == 0 {
                    Self::merge_items(self.root.into_items(), other.root.into_items())
                } else {
                    Self::merge_nodes(self.height, self.root.into_nodes(), other.root.into_nodes())
                }
            }
            Ordering::Greater => {
                let mut nodes = self.root.into_nodes();
                if other.height == self.height - 1 && other.root.is_at_least_half_full() {
                    return Self::append_node(self.height, nodes, other.root);
                }
                let tree = Self::concat(
                    Self {
                        height: self.height - 1,
                        root: Arc::make_mut(&mut nodes).pop().unwrap(),
                    },
                    other,
                );
                if tree.height == self.height - 1 {
                    Self::append_node(self.height, nodes, tree.root)
                } else {
                    Self::merge_nodes(self.height, nodes, tree.root.into_nodes())
                }
            }
        }
    }

    pub fn push_front(self, item: T) -> Self {
        if self.height == 0 {
            return Self::prepend_item(item, self.root.into_items());
        }
        let mut nodes = self.root.into_nodes();
        let node = Arc::make_mut(&mut nodes).remove(0);
        let tree = Self {
            height: self.height - 1,
            root: node,
        }
        .push_front(item);
        if tree.height == self.height - 1 {
            Self::prepend_node(self.height, tree.root, nodes)
        } else {
            Self::merge_nodes(self.height, tree.root.into_nodes(), nodes)
        }
    }

    pub fn push_back(self, item: T) -> Self {
        if self.height == 0 {
            return Self::append_item(self.root.into_items(), item);
        }
        let mut nodes = self.root.into_nodes();
        let node = Arc::make_mut(&mut nodes).pop().unwrap();
        let tree = Self {
            height: self.height - 1,
            root: node,
        }
        .push_back(item);
        if tree.height == self.height - 1 {
            Self::append_node(self.height, nodes, tree.root)
        } else {
            Self::merge_nodes(self.height, nodes, tree.root.into_nodes())
        }
    }

    fn merge_items(mut items_0: Arc<Vec<T>>, mut items_1: Arc<Vec<T>>) -> Self {
        use std::cmp::Ordering;

        if items_0.len() >= MIN_ITEM_COUNT && items_1.len() >= MIN_ITEM_COUNT {
            return Self {
                height: 1,
                root: Node::from_nodes(Arc::new(vec![
                    Node::from_items(items_0),
                    Node::from_items(items_1),
                ])),
            };
        }
        let count = items_0.len() + items_1.len();
        if count <= MAX_ITEM_COUNT {
            let items_0_mut = Arc::make_mut(&mut items_0);
            let items_1_mut = Arc::make_mut(&mut items_1);
            items_0_mut.extend(items_1_mut.drain(..));
            Self {
                height: 0,
                root: Node::from_items(items_0),
            }
        } else {
            match items_0.len().cmp(&items_1.len()) {
                Ordering::Less => {
                    let index = (items_1.len() - items_0.len()) / 2;
                    let items_0_mut = Arc::make_mut(&mut items_0);
                    let items_1_mut = Arc::make_mut(&mut items_1);
                    items_0_mut.extend(items_1_mut.drain(..index));
                }
                Ordering::Greater => {
                    let index = (items_0.len() + items_1.len()) / 2;
                    let items_0_mut = Arc::make_mut(&mut items_0);
                    let items_1_mut = Arc::make_mut(&mut items_1);
                    items_1_mut.splice(..0, items_0_mut.drain(index..));
                }
                Ordering::Equal => {}
            }
            Self {
                height: 1,
                root: Node::from_nodes(Arc::new(vec![
                    Node::from_items(items_0),
                    Node::from_items(items_1),
                ])),
            }
        }
    }

    fn merge_nodes(
        height: usize,
        mut nodes_0: Arc<Vec<Node<T, Tag>>>,
        mut nodes_1: Arc<Vec<Node<T, Tag>>>,
    ) -> Self {
        use std::cmp::Ordering;

        if nodes_0.len() >= MIN_ITEM_COUNT && nodes_1.len() >= MIN_ITEM_COUNT {
            return Self {
                height: height + 1,
                root: Node::from_nodes(Arc::new(vec![
                    Node::from_nodes(nodes_0),
                    Node::from_nodes(nodes_1),
                ])),
            };
        }
        let count = nodes_0.len() + nodes_1.len();
        if count <= MAX_NODE_COUNT {
            let nodes_0_mut = Arc::make_mut(&mut nodes_0);
            let nodes_1_mut = Arc::make_mut(&mut nodes_1);
            nodes_0_mut.extend(nodes_1_mut.drain(..));
            Self {
                height,
                root: Node::from_nodes(nodes_0),
            }
        } else {
            match nodes_0.len().cmp(&nodes_1.len()) {
                Ordering::Less => {
                    let index = (nodes_1.len() - nodes_0.len()) / 2;
                    let nodes_0_mut = Arc::make_mut(&mut nodes_0);
                    let nodes_1_mut = Arc::make_mut(&mut nodes_1);
                    nodes_0_mut.extend(nodes_1_mut.drain(..index));
                }
                Ordering::Greater => {
                    let index = (nodes_0.len() + nodes_1.len()) / 2;
                    let nodes_0_mut = Arc::make_mut(&mut nodes_0);
                    let nodes_1_mut = Arc::make_mut(&mut nodes_1);
                    nodes_1_mut.splice(..0, nodes_0_mut.drain(index..));
                }
                Ordering::Equal => {}
            }
            Self {
                height: height + 1,
                root: Node::from_nodes(Arc::new(vec![
                    Node::from_nodes(nodes_0),
                    Node::from_nodes(nodes_1),
                ])),
            }
        }
    }

    fn prepend_item(item: T, mut items: Arc<Vec<T>>) -> Self {
        use std::iter;

        if items.len() + 1 <= MAX_ITEM_COUNT {
            Arc::make_mut(&mut items).insert(0, item);
            return Self {
                height: 0,
                root: Node::from_items(items),
            };
        }
        let other_items = iter::once(item)
            .chain(Arc::make_mut(&mut items).drain(..MIN_ITEM_COUNT))
            .collect::<Vec<_>>();
        Self {
            height: 1,
            root: Node::from_nodes(Arc::new(vec![
                Node::from_items(Arc::new(other_items)),
                Node::from_items(items),
            ])),
        }
    }

    fn prepend_node(height: usize, node: Node<T, Tag>, mut nodes: Arc<Vec<Node<T, Tag>>>) -> Self {
        use std::iter;

        if nodes.len() + 1 <= MAX_NODE_COUNT {
            Arc::make_mut(&mut nodes).insert(0, node);
            return Self {
                height,
                root: Node::from_nodes(nodes),
            };
        }
        let other_nodes = iter::once(node)
            .chain(Arc::make_mut(&mut nodes).drain(..MIN_NODE_COUNT))
            .collect::<Vec<_>>();
        Self {
            height: height + 1,
            root: Node::from_nodes(Arc::new(vec![
                Node::from_nodes(Arc::new(other_nodes)),
                Node::from_nodes(nodes),
            ])),
        }
    }

    fn append_item(mut items: Arc<Vec<T>>, item: T) -> Self {
        use std::iter;

        if items.len() + 1 <= MAX_ITEM_COUNT {
            Arc::make_mut(&mut items).push(item);
            return Self {
                height: 0,
                root: Node::from_items(items),
            };
        }
        let other_items = Arc::make_mut(&mut items)
            .drain(MIN_ITEM_COUNT..)
            .chain(iter::once(item))
            .collect::<Vec<_>>();
        Self {
            height: 1,
            root: Node::from_nodes(Arc::new(vec![
                Node::from_items(items),
                Node::from_items(Arc::new(other_items)),
            ])),
        }
    }

    fn append_node(height: usize, mut nodes: Arc<Vec<Node<T, Tag>>>, node: Node<T, Tag>) -> Self {
        use std::iter;

        if nodes.len() + 1 <= MAX_NODE_COUNT {
            Arc::make_mut(&mut nodes).push(node);
            return Self {
                height,
                root: Node::from_nodes(nodes),
            };
        }
        let other_nodes = Arc::make_mut(&mut nodes)
            .drain(MIN_NODE_COUNT..)
            .chain(iter::once(node))
            .collect::<Vec<_>>();
        Self {
            height: height + 1,
            root: Node::from_nodes(Arc::new(vec![
                Node::from_nodes(nodes),
                Node::from_nodes(Arc::new(other_nodes)),
            ])),
        }
    }
}

impl<T: Eq, Tag> Eq for BTree<T, Tag> {}

impl<'a, T: Clone, Tag: Clone + Measure<T> + Monoid> From<&'a [T]> for BTree<T, Tag> {
    fn from(items: &'a [T]) -> Self {
        Self::from(items.to_vec())
    }
}

impl<T: Clone, Tag: Clone + Measure<T> + Monoid> From<Vec<T>> for BTree<T, Tag> {
    fn from(items: Vec<T>) -> Self {
        if items.len() < MAX_ITEM_COUNT {
            return Self {
                height: 0,
                root: Node::from_items(Arc::new(items)),
            };
        }
        items.into_iter().collect()
    }
}

impl<T: Clone, Tag: Clone + Measure<T> + Monoid> FromIterator<T> for BTree<T, Tag> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut builder = Builder::new();
        for item in iter.into_iter() {
            builder.push(item);
        }
        builder.build()
    }
}

impl<T, Tag> Index<usize> for BTree<T, Tag> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).unwrap()
    }
}

impl<'a, T, Tag> IntoIterator for &'a BTree<T, Tag> {
    type IntoIter = Iter<'a, T, Tag>;
    type Item = &'a T;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T: PartialEq, Tag> PartialEq for BTree<T, Tag> {
    fn eq(&self, other: &Self) -> bool {
        if self.height != other.height {
            return false;
        }
        self.root == other.root
    }
}

pub trait Measure<T> {
    fn measure(item: &T) -> Self;
}

impl<T> Measure<T> for () {
    fn measure(_item: &T) -> Self {
        ()
    }
}

pub trait Monoid {
    fn empty() -> Self;

    fn append(self, other: Self) -> Self;
}

impl Monoid for () {
    fn empty() -> Self {
        ()
    }

    fn append(self, _other: Self) -> Self {
        ()
    }
}

#[derive(Clone, Debug)]
struct Node<T, Tag> {
    tag: Tag,
    kind: NodeKind<T, Tag>,
}

#[derive(Clone, Debug)]
enum NodeKind<T, Tag> {
    Leaf {
        items: Arc<Vec<T>>,
    },
    Branch {
        len: usize,
        nodes: Arc<Vec<Node<T, Tag>>>,
    },
}

impl<T, Tag> Node<T, Tag> {
    fn is_at_least_half_full(&self) -> bool {
        match &self.kind {
            NodeKind::Leaf { items } => items.len() >= MIN_ITEM_COUNT,
            NodeKind::Branch { nodes, .. } => nodes.len() >= MIN_NODE_COUNT,
        }
    }

    fn len(&self) -> usize {
        match &self.kind {
            NodeKind::Leaf { items } => items.len(),
            NodeKind::Branch { len, .. } => *len,
        }
    }

    fn into_items(self) -> Arc<Vec<T>> {
        match self.kind {
            NodeKind::Leaf { items } => items,
            _ => panic!(),
        }
    }

    fn into_nodes(self) -> Arc<Vec<Self>> {
        match self.kind {
            NodeKind::Branch { nodes, .. } => nodes,
            _ => panic!(),
        }
    }
}

impl<T, Tag: Monoid> Node<T, Tag> {
    fn new() -> Self {
        Self {
            tag: Tag::empty(),
            kind: NodeKind::Leaf {
                items: Arc::new(vec![]),
            },
        }
    }
}

impl<T, Tag: Measure<T> + Monoid> Node<T, Tag> {
    fn from_items(items: Arc<Vec<T>>) -> Self {
        debug_assert!(items.len() <= MAX_ITEM_COUNT);
        Self {
            tag: items
                .iter()
                .map(|item| Tag::measure(item))
                .fold(Tag::empty(), |tag_0, tag_1| tag_0.append(tag_1)),
            kind: NodeKind::Leaf { items },
        }
    }
}

impl<T, Tag: Clone + Monoid> Node<T, Tag> {
    fn from_nodes(nodes: Arc<Vec<Self>>) -> Self {
        debug_assert!(nodes.len() <= MAX_NODE_COUNT);
        debug_assert!(nodes.iter().all(|node| node.is_at_least_half_full()));
        Self {
            tag: nodes
                .iter()
                .map(|node| node.tag.clone())
                .fold(Tag::empty(), |tag_0, tag_1| tag_0.append(tag_1)),
            kind: NodeKind::Branch {
                len: nodes.iter().map(|node| node.len()).sum(),
                nodes,
            },
        }
    }
}

impl<T: Eq, Tag> Eq for Node<T, Tag> {}

impl<T: PartialEq, Tag> PartialEq for Node<T, Tag> {
    fn eq(&self, other: &Self) -> bool {
        match (&self.kind, &other.kind) {
            (NodeKind::Leaf { items }, NodeKind::Leaf { items: other_items }) => {
                if Arc::ptr_eq(items, other_items) {
                    return true;
                }
                items == other_items
            }
            (
                NodeKind::Branch { nodes, .. },
                NodeKind::Branch {
                    nodes: other_nodes, ..
                },
            ) => {
                if Arc::ptr_eq(nodes, other_nodes) {
                    return true;
                }
                nodes == other_nodes
            }
            _ => panic!(),
        }
    }
}

pub struct Builder<T, Tag: Measure<T>> {
    level_stack: Vec<Level<T, Tag>>,
    items: Vec<T>,
}

impl<T: Clone, Tag: Clone + Measure<T> + Monoid> Builder<T, Tag> {
    pub fn new() -> Self {
        Self {
            level_stack: Vec::new(),
            items: Vec::new(),
        }
    }

    pub fn push(&mut self, item: T) {
        self.items.push(item);
        if self.items.len() < MAX_ITEM_COUNT {
            return;
        }
        let mut height = 0;
        let mut node = Node::from_items(Arc::new(mem::replace(&mut self.items, Vec::new())));
        loop {
            if self
                .level_stack
                .last()
                .map_or(true, |level| level.height != height)
            {
                self.level_stack.push(Level {
                    height,
                    nodes: Vec::new(),
                })
            }
            self.level_stack.last_mut().unwrap().nodes.push(node);
            if self.level_stack.last_mut().unwrap().nodes.len() < MAX_NODE_COUNT {
                break;
            }
            height += 1;
            node = Node::from_nodes(Arc::new(self.level_stack.pop().unwrap().nodes))
        }
    }

    pub fn build(mut self) -> BTree<T, Tag> {
        let mut tree = BTree {
            height: 0,
            root: Node::from_items(Arc::new(self.items)),
        };
        while let Some(level) = self.level_stack.pop() {
            let height = level.height;
            tree = level
                .nodes
                .into_iter()
                .rev()
                .fold(tree, |tree, node| BTree { height, root: node }.concat(tree))
        }
        tree
    }
}

struct Level<T, Tag> {
    height: usize,
    nodes: Vec<Node<T, Tag>>,
}

pub struct Iter<'a, T, Tag> {
    count: usize,
    cursor: Cursor<'a, T, Tag>,
}

impl<'a, T, Tag> Iterator for Iter<'a, T, Tag> {
    type Item = &'a T;

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.count, Some(self.count))
    }

    fn next(&mut self) -> Option<Self::Item> {
        if self.count == 0 {
            return None;
        }
        self.count -= 1;
        let item = self.cursor.get();
        self.cursor.move_next();
        Some(item)
    }
}

impl<'a, T, Tag> DoubleEndedIterator for Iter<'a, T, Tag> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.count == 0 {
            return None;
        }
        self.count -= 1;
        self.cursor.move_previous();
        Some(self.cursor.get())
    }
}

impl<'a, T, Tag> ExactSizeIterator for Iter<'a, T, Tag> {}

pub struct Range<'a, T, Tag> {
    count: usize,
    front: Cursor<'a, T, Tag>,
    back: Cursor<'a, T, Tag>,
}

impl<'a, T, Tag> Iterator for Range<'a, T, Tag> {
    type Item = &'a T;

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.count, Some(self.count))
    }

    fn next(&mut self) -> Option<Self::Item> {
        if self.count == 0 {
            return None;
        }
        self.count -= 1;
        let item = self.front.get();
        self.front.move_next();
        Some(item)
    }
}

impl<'a, T, Tag> DoubleEndedIterator for Range<'a, T, Tag> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.count == 0 {
            return None;
        }
        self.count -= 1;
        self.back.move_previous();
        Some(self.back.get())
    }
}

impl<'a, T, Tag> ExactSizeIterator for Range<'a, T, Tag> {}

struct Cursor<'a, T, Tag> {
    root: &'a Node<T, Tag>,
    node_stack: Vec<(&'a [Node<T, Tag>], usize)>,
    item: Option<(&'a [T], usize)>,
}

impl<'a, T, Tag> Cursor<'a, T, Tag> {
    fn front(root: &'a Node<T, Tag>) -> Self {
        let mut cursor = Self {
            root,
            node_stack: Vec::new(),
            item: None,
        };
        cursor.descend_left();
        cursor
    }

    fn back(root: &'a Node<T, Tag>) -> Self {
        Self {
            root,
            node_stack: Vec::new(),
            item: None,
        }
    }

    fn at(root: &'a Node<T, Tag>, index: usize) -> Self {
        let mut cursor = Self {
            root,
            node_stack: Vec::new(),
            item: None,
        };
        cursor.descend_to(index);
        cursor
    }

    fn get(&self) -> &'a T {
        self.item.map(|(items, index)| &items[index]).unwrap()
    }

    fn move_next(&mut self) {
        if let Some((items, index)) = self.item.as_mut() {
            if *index < items.len() - 1 {
                *index += 1;
                return;
            }
            self.item = None;
        }
        while let Some((nodes, index)) = self.node_stack.last_mut() {
            if *index < nodes.len() - 1 {
                *index += 1;
                break;
            }
            self.node_stack.pop();
        }
        self.descend_left()
    }

    fn move_previous(&mut self) {
        if let Some((_, index)) = self.item.as_mut() {
            if *index > 0 {
                *index -= 1;
                return;
            }
            self.item = None;
        }
        while let Some((_, index)) = self.node_stack.last_mut() {
            if *index > 0 {
                *index -= 1;
                break;
            }
            self.node_stack.pop();
        }
        self.descend_right();
    }

    fn descend_left(&mut self) {
        loop {
            match &self
                .node_stack
                .last()
                .map(|(nodes, index)| &nodes[*index])
                .unwrap_or(&self.root)
                .kind
            {
                NodeKind::Leaf { items } => {
                    if !items.is_empty() {
                        self.item = Some((items, 0));
                    }
                    break;
                }
                NodeKind::Branch { nodes, .. } => self.node_stack.push((&*nodes, 0)),
            }
        }
    }

    fn descend_right(&mut self) {
        loop {
            match &self
                .node_stack
                .last()
                .map(|(nodes, index)| &nodes[*index])
                .unwrap_or(&self.root)
                .kind
            {
                NodeKind::Leaf { items } => {
                    if !items.is_empty() {
                        let index = items.len() - 1;
                        self.item = Some((items, index));
                    }
                    break;
                }
                NodeKind::Branch { nodes, .. } => self.node_stack.push((&*nodes, nodes.len() - 1)),
            }
        }
    }

    fn descend_to(&mut self, mut index: usize) {
        loop {
            match &self
                .node_stack
                .last()
                .map(|(nodes, index)| &nodes[*index])
                .unwrap_or(&self.root)
                .kind
            {
                NodeKind::Leaf { items } => {
                    if !items.is_empty() {
                        self.item = Some((items, index));
                    }
                    break;
                }
                NodeKind::Branch { nodes, .. } => {
                    let (index_0, index_1) = search(nodes, index);
                    self.node_stack.push((&*nodes, index_0));
                    index = index_1;
                }
            }
        }
    }
}

fn check_bounds<R: RangeBounds<usize>>(range: R, len: usize) -> ops::Range<usize> {
    let start = match range.start_bound() {
        Bound::Included(start) => *start,
        Bound::Excluded(start) => start.checked_add(1).unwrap(),
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Included(end) => end.checked_add(1).unwrap(),
        Bound::Excluded(end) => *end,
        Bound::Unbounded => len,
    };
    assert!(start <= end);
    assert!(end <= len);
    ops::Range { start, end }
}

fn search<T, Tag>(nodes: &[Node<T, Tag>], mut index: usize) -> (usize, usize) {
    (
        nodes
            .iter()
            .position(|node| {
                if index < node.len() {
                    return true;
                }
                index -= node.len();
                false
            })
            .unwrap(),
        index,
    )
}

#[cfg(test)]
mod tests {
    use {super::*, proptest::prelude::*, std::fmt};

    impl<
            T: Arbitrary + fmt::Debug + 'static,
            Tag: Clone + fmt::Debug + Measure<T> + Monoid + 'static,
        > Arbitrary for BTree<T, Tag>
    {
        type Parameters = usize;
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with(max_height: Self::Parameters) -> Self::Strategy {
            fn arbitrary_node<
                T: Arbitrary + fmt::Debug + 'static,
                Tag: Clone + fmt::Debug + Measure<T> + Monoid + 'static,
            >(
                height: usize,
            ) -> impl Strategy<Value = Node<T, Tag>> {
                if height == 0 {
                    prop::collection::vec(any::<T>(), MIN_ITEM_COUNT..=MAX_ITEM_COUNT)
                        .prop_map(|items| Node::from_items(Arc::new(items)))
                        .boxed()
                } else {
                    prop::collection::vec(
                        arbitrary_node(height - 1),
                        MIN_NODE_COUNT..MAX_NODE_COUNT,
                    )
                    .prop_map(|nodes| Node::from_nodes(Arc::new(nodes)))
                    .boxed()
                }
            }

            (0..max_height)
                .prop_flat_map(|height| {
                    arbitrary_node(height).prop_map(move |root| BTree { height, root })
                })
                .boxed()
        }
    }

    #[derive(Clone, Debug)]
    struct EvenCount(usize);

    impl Measure<u16> for EvenCount {
        fn measure(item: &u16) -> Self {
            Self(if item % 2 == 0 { 1 } else { 0 })
        }
    }

    impl Monoid for EvenCount {
        fn empty() -> Self {
            Self(0)
        }

        fn append(self, other: Self) -> Self {
            Self(self.0 + other.0)
        }
    }

    proptest! {
        #[test]
        fn len(tree in any_with::<BTree<u16, ()>>(8),) {
            assert_eq!(tree.len(), tree.iter().count());
        }

        #[test]
        fn tag(tree in any_with::<BTree<u16, EvenCount>>(8),) {
            assert_eq!(tree.tag().0, tree.iter().filter(|item| *item % 2 == 0).count());
        }

        #[test]
        fn front(tree in any_with::<BTree<u16, ()>>(8),) {
            assert_eq!(tree.front(), tree.iter().next());
        }

        #[test]
        fn back(tree in any_with::<BTree<u16, ()>>(8),) {
            assert_eq!(tree.back(), tree.iter().rev().next());
        }

        #[test]
        fn get((tree, index) in any_with::<BTree::<u16, ()>>(8).prop_flat_map(|tree| {
            let len = tree.len();
            (Just(tree), 0..len)
        })) {
            assert_eq!(tree.get(index).cloned(), tree.iter().nth(index).cloned());
        }

        #[test]
        fn iter(vec in prop::collection::vec(any::<u16>(), 256)) {
            let tree = BTree::<u16, ()>::from(vec.clone());
            assert_eq!(
                tree.iter().cloned().collect::<Vec<_>>(),
                vec
            );
            assert_eq!(
                tree.iter().cloned().rev().collect::<Vec<_>>(),
                vec.iter().cloned().rev().collect::<Vec<_>>()
            );
        }

        #[test]
        fn range((tree, (start, end)) in any_with::<BTree::<u16, ()>>(1).prop_flat_map(|tree| {
            let len = tree.len();
            (
                Just(tree),
                (0..len).prop_flat_map(move |start| (Just(start), start..len))
            )
        })) {
            assert_eq!(
                tree.range(start..end).cloned().collect::<Vec<_>>(),
                tree.iter().skip(start).take(end - start).cloned().collect::<Vec<_>>()
            );
            assert_eq!(
                tree.range(start..end).cloned().rev().collect::<Vec<_>>(),
                tree.iter().skip(start).take(end - start).cloned().rev().collect::<Vec<_>>()
            )
        }

        #[test]
        fn splice(
            ((tree_0, (start, end)), tree_1) in (any_with::<BTree::<u16, ()>>(8).prop_flat_map(|tree| {
                let len = tree.len();
                (
                    Just(tree),
                    (0..=len).prop_flat_map(move |start| (Just(start), start..=len))
                )
            }), any_with::<BTree::<u16, ()>>(8))
        ) {
            let new_tree = tree_0.clone().splice(start..end, tree_1.clone());
            assert_eq!(
                new_tree.iter().cloned().collect::<Vec<_>>(),
                tree_0
                    .iter()
                    .take(start)
                    .chain(tree_1.iter()).chain(tree_0.iter().skip(end))
                    .cloned()
                    .collect::<Vec<_>>()
            );
        }

        #[test]
        fn insert(
            ((tree, index), item) in (any_with::<BTree::<u16, ()>>(8).prop_flat_map(|tree| {
                let len = tree.len();
                (Just(tree), 0..=len)
            }), any::<u16>()))
        {
            use std::iter;

            let new_tree = tree.clone().insert(index, item);
            assert_eq!(
                new_tree.iter().cloned().collect::<Vec<_>>(),
                tree
                    .iter()
                    .take(index)
                    .cloned()
                    .chain(iter::once(item))
                    .chain(tree.iter().skip(index).cloned())
                    .collect::<Vec<_>>()
            );
        }

        #[test]
        fn remove(
            (tree, index) in (any_with::<BTree::<u16, ()>>(8).prop_flat_map(|tree| {
                let len = tree.len();
                (Just(tree), 0..len)
            }))
        ) {
            let new_tree = tree.clone().remove(index);
            assert_eq!(
                new_tree.iter().cloned().collect::<Vec<_>>(),
                tree
                    .iter()
                    .take(index)
                    .chain(tree.iter().skip(index + 1))
                    .cloned()
                    .collect::<Vec<_>>()
            );
        }

        #[test]
        fn split(
            (tree, index) in any_with::<BTree::<u16, ()>>(8).prop_flat_map(|tree| {
                let len = tree.len();
                (Just(tree), 0..len)
            })
        ) {
            let (tree_0, tree_1) = tree.clone().split(index);
            assert_eq!(
                tree_0.iter().chain(tree_1.iter()).cloned().collect::<Vec<_>>(),
                tree.iter().cloned().collect::<Vec<_>>()
            );
        }

        #[test]
        fn truncate_before(
            (tree, index) in any_with::<BTree::<u16, ()>>(8).prop_flat_map(|tree| {
                let len = tree.len();
                (Just(tree), 0..len)
            })
        ) {
            let new_tree = tree.clone().truncate_before(index);
            assert_eq!(
                new_tree.iter().cloned().collect::<Vec<_>>(),
                tree.iter().cloned().skip(index).collect::<Vec<_>>()
            );
        }

        #[test]
        fn truncate_after(
            (tree, index) in any_with::<BTree::<u16, ()>>(8).prop_flat_map(|tree| {
                let len = tree.len();
                (Just(tree), 0..len)
            })
        ) {
            let new_tree = tree.clone().truncate_after(index);
            assert_eq!(
                new_tree.iter().cloned().collect::<Vec<_>>(),
                tree.iter().cloned().take(index).collect::<Vec<_>>()
            );
        }

        #[test]
        fn pop_front(tree in any_with::<BTree::<u16, ()>>(8)) {
            let new_tree = tree.clone().pop_front();
            assert_eq!(
                new_tree.iter().cloned().collect::<Vec<_>>(),
                tree.iter().cloned().skip(1).collect::<Vec<_>>(),
            )
        }

        #[test]
        fn pop_back(tree in any_with::<BTree::<u16, ()>>(8)) {
            let new_tree = tree.clone().pop_back();
            assert_eq!(
                new_tree.iter().cloned().collect::<Vec<_>>(),
                tree.iter().cloned().take(tree.len() - 1).collect::<Vec<_>>(),
            )
        }

        #[test]
        fn concat(
            tree_0 in any_with::<BTree::<u16, ()>>(8),
            tree_1 in any_with::<BTree::<u16, ()>>(8),
        ) {
            let tree = tree_0.clone().concat(tree_1.clone());
            assert_eq!(
                tree.iter().cloned().collect::<Vec<_>>(),
                tree_0.iter().chain(tree_1.iter()).cloned().collect::<Vec<_>>()
            )
        }

        #[test]
        fn push_front(
            tree in any_with::<BTree<u16, ()>>(8),
            item in any::<u16>()
        ) {
            use std::iter;

            let new_tree = tree.clone().push_front(item);
            assert_eq!(
                new_tree.iter().cloned().collect::<Vec<_>>(),
                iter::once(item).chain(tree.iter().cloned()).collect::<Vec<_>>()
            );
        }

        #[test]
        fn push_back(
            tree in any_with::<BTree<u16, ()>>(8),
            item in any::<u16>()
        ) {
            use std::iter;

            let new_tree = tree.clone().push_back(item);
            assert_eq!(
                new_tree.iter().cloned().collect::<Vec<_>>(),
                tree.iter().cloned().chain(iter::once(item)).collect::<Vec<_>>()
            );
        }
    }
}

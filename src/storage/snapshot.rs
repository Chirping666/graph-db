//! Snapshot: a consistent point-in-time view of the database.
//!
//! A snapshot captures the set of B-tree root page IDs and transaction
//! ID at a particular commit. Read transactions operate against a
//! snapshot; write transactions produce a new one.

use super::format::Superblock;
use super::page::PageId;

/// The set of B-tree root page IDs that define a database state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotRoots {
    /// Node Store B-tree root.
    pub node_store: PageId,
    /// Edge Store B-tree root.
    pub edge_store: PageId,
    /// Outgoing Adjacency Index root.
    pub outgoing_adj: PageId,
    /// Incoming Adjacency Index root.
    pub incoming_adj: PageId,
    /// Type Index root.
    pub type_index: PageId,
    /// Schema Store root.
    pub schema_store: PageId,
    /// ID Freelist root.
    pub id_freelist: PageId,
    /// Page Freelist root.
    pub page_freelist: PageId,
}

/// A consistent point-in-time view of the database, defined by a set
/// of B-tree root page IDs and a transaction ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The transaction ID at which this snapshot was taken.
    pub transaction_id: u64,
    /// Total number of pages in the file at this snapshot.
    pub total_pages: u64,
    /// The root page IDs for all 8 B-trees.
    pub roots: SnapshotRoots,
}

impl Snapshot {
    /// Returns the root page ID for the B-tree at the given catalog index,
    /// or `None` if `tree_index >= 8`.
    ///
    /// Index mapping (per `012-design-document.md` §19.1):
    /// - 0: Node Store
    /// - 1: Edge Store
    /// - 2: Outgoing Adjacency
    /// - 3: Incoming Adjacency
    /// - 4: Type Index
    /// - 5: Schema Store
    /// - 6: ID Freelist
    /// - 7: Page Freelist
    pub fn root_for_tree(&self, tree_index: usize) -> Option<PageId> {
        match tree_index {
            0 => Some(self.roots.node_store),
            1 => Some(self.roots.edge_store),
            2 => Some(self.roots.outgoing_adj),
            3 => Some(self.roots.incoming_adj),
            4 => Some(self.roots.type_index),
            5 => Some(self.roots.schema_store),
            6 => Some(self.roots.id_freelist),
            7 => Some(self.roots.page_freelist),
            _ => None,
        }
    }
}

impl From<&Superblock> for Snapshot {
    fn from(sb: &Superblock) -> Self {
        Self {
            transaction_id: sb.transaction_id,
            total_pages: sb.total_pages,
            roots: SnapshotRoots {
                node_store: sb.root_node_store,
                edge_store: sb.root_edge_store,
                outgoing_adj: sb.root_outgoing_adj,
                incoming_adj: sb.root_incoming_adj,
                type_index: sb.root_type_index,
                schema_store: sb.root_schema_store,
                id_freelist: sb.root_id_freelist,
                page_freelist: sb.root_page_freelist,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_superblock() -> Superblock {
        Superblock {
            transaction_id: 5,
            total_pages: 100,
            feature_flags: 0,
            root_node_store: PageId(10),
            root_edge_store: PageId(11),
            root_outgoing_adj: PageId(12),
            root_incoming_adj: PageId(13),
            root_type_index: PageId(14),
            root_schema_store: PageId(15),
            root_id_freelist: PageId(16),
            root_page_freelist: PageId(17),
            checksum: 0,
        }
    }

    #[test]
    fn from_superblock() {
        let sb = test_superblock();
        let snap = Snapshot::from(&sb);
        assert_eq!(snap.transaction_id, 5);
        assert_eq!(snap.total_pages, 100);
        assert_eq!(snap.roots.node_store, PageId(10));
        assert_eq!(snap.roots.edge_store, PageId(11));
        assert_eq!(snap.roots.outgoing_adj, PageId(12));
        assert_eq!(snap.roots.incoming_adj, PageId(13));
        assert_eq!(snap.roots.type_index, PageId(14));
        assert_eq!(snap.roots.schema_store, PageId(15));
        assert_eq!(snap.roots.id_freelist, PageId(16));
        assert_eq!(snap.roots.page_freelist, PageId(17));
    }

    #[test]
    fn root_for_tree_mapping() {
        let snap = Snapshot::from(&test_superblock());
        assert_eq!(snap.root_for_tree(0), Some(PageId(10))); // node_store
        assert_eq!(snap.root_for_tree(5), Some(PageId(15))); // schema_store
        assert_eq!(snap.root_for_tree(7), Some(PageId(17))); // page_freelist
    }

    #[test]
    fn root_for_tree_out_of_range() {
        let snap = Snapshot::from(&test_superblock());
        assert_eq!(snap.root_for_tree(8), None);
        assert_eq!(snap.root_for_tree(usize::MAX), None);
    }
}

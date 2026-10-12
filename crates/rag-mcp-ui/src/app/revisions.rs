//! Revision history workspace: list, diff and restore.

use super::*;

impl GraphApp {
    pub(super) fn open_revisions(&mut self, item: &LibraryItem) {
        if self.pending_revision_restore.is_some() {
            self.mode = ViewMode::Revisions;
            self.revision.error = Some(
                "Идёт восстановление. Дождитесь результата перед открытием другой истории.".into(),
            );
            return;
        }
        self.revision.clear();
        self.revision.document_id = Some(item.id.clone());
        self.revision.document_title = item.title.clone();
        self.revision.document_uri = item.uri.clone();
        self.revision.document_layer = item.layer.clone();
        self.revision.head = self
            .library_document
            .as_ref()
            .filter(|body| body.id == item.id)
            .and_then(|body| body.revision);
        self.mode = ViewMode::Revisions;
        self.reload_revisions();
    }

    pub(super) fn reload_revisions(&mut self) {
        self.revision.selected = None;
        self.revision.snapshot = None;
        self.revision.snapshot_error = None;
        self.revision.diff = None;
        self.revision.diff_error = None;
        self.revision.restore_confirm = None;
        self.request_revisions(None, false);
    }

    pub(super) fn load_more_revisions(&mut self) {
        let Some(cursor) = self.revision.next_cursor.clone() else {
            return;
        };
        self.request_revisions(Some(cursor), true);
    }

    pub(super) fn request_revisions(&mut self, cursor: Option<String>, append: bool) {
        let Some(document_id) = self.revision.document_id.clone() else {
            self.revision.error = Some("Откройте историю из документа в корпусе".into());
            return;
        };
        let Some(base) = self.activity_base() else {
            self.revision.error = Some("Для истории версий требуется HTTP gateway".into());
            return;
        };
        self.revision.error = None;
        if !append {
            self.revision.next_cursor = None;
        }
        let seq = self.next_seq();
        self.pending_revisions = Some(seq);
        self.worker.send(WorkerCmd::LoadRevisions {
            seq,
            base,
            document_id,
            cursor,
            append,
        });
    }

    pub(super) fn load_revision_details(&mut self, from_revision: i64) {
        let Some(document_id) = self.revision.document_id.clone() else {
            return;
        };
        let Some(base) = self.activity_base() else {
            self.revision.error = Some("Для деталей версии требуется HTTP gateway".into());
            return;
        };
        self.revision.selected = Some(from_revision);
        self.revision.snapshot = None;
        self.revision.snapshot_error = None;
        self.revision.diff = None;
        self.revision.diff_error = None;
        let snapshot_seq = self.next_seq();
        self.pending_revision_snapshot = Some(snapshot_seq);
        self.worker.send(WorkerCmd::LoadRevisionSnapshot {
            seq: snapshot_seq,
            base: base.clone(),
            document_id: document_id.clone(),
            revision: from_revision,
        });
        let diff_seq = self.next_seq();
        self.pending_revision_diff = Some(diff_seq);
        self.worker.send(WorkerCmd::LoadRevisionDiff {
            seq: diff_seq,
            base,
            document_id,
            from_revision,
            to_revision: None,
        });
    }

    pub(super) fn restore_revision(&mut self, revision: i64) {
        let selected_is_raw = self
            .revision
            .items
            .iter()
            .find(|item| item.revision == revision)
            .is_some_and(|item| item.layer == "raw");
        if self.revision.document_layer == "raw" || selected_is_raw {
            self.revision.error = Some(
                "Raw-документы управляются исходным файлом. Восстановите файл и синхронизируйте его, чтобы создать новую индексированную версию."
                    .into(),
            );
            self.revision.restore_confirm = None;
            return;
        }
        let (Some(document_id), Some(if_match_revision), Some(base)) = (
            self.revision.document_id.clone(),
            self.revision.head,
            self.activity_base(),
        ) else {
            self.revision.error =
                Some("Для восстановления нужны текущая версия документа и HTTP gateway".into());
            return;
        };
        self.revision.error = None;
        let seq = self.next_seq();
        self.pending_revision_restore = Some(seq);
        self.worker.send(WorkerCmd::RestoreRevision {
            seq,
            base,
            document_id,
            revision,
            if_match_revision,
        });
    }
}

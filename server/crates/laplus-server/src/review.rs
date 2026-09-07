//! The sidebar's live Working tree and Branch changes previews.
//!
//! This is independent of the provider and of saved-turn checkpoints. The wire
//! shape is `packages/contracts/src/review.ts`; Git execution, private-index
//! snapshots and bounded patch reading are the existing backend's.

use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::checkpoints::{self, Patch};
use crate::git::{self, Unavailable};
use crate::projects::WorkspaceRoot;

pub const GET_DIFF_PREVIEW: &str = "review.getDiffPreview";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffPreview {
    cwd: String,
    base_ref: Option<String>,
    ignore_whitespace: Option<bool>,
}

impl DiffPreview {
    pub fn read(payload: &Value) -> Result<Self, Value> {
        let mut call: Self = serde_json::from_value(payload.clone())
            .map_err(|why| Unavailable::malformed(why).to_error(GET_DIFF_PREVIEW, ""))?;
        call.cwd =
            git::workspace(&call.cwd).map_err(|why| why.to_error(GET_DIFF_PREVIEW, &call.cwd))?;
        if let Some(base) = &mut call.base_ref {
            *base = base.trim().to_string();
            if base.is_empty() || base.starts_with('-') {
                return Err(Unavailable::Unusable {
                    detail: "The comparison base must name a Git revision, not an empty value or an option.".to_string(),
                }.to_error(GET_DIFF_PREVIEW, &call.cwd));
            }
        }
        Ok(call)
    }

    /// Runs as deferred work: directory checks and every Git child stay off the
    /// socket reader, just like the existing checkpoint diff methods.
    pub fn run(self) -> Result<Value, Value> {
        self.preview()
            .map_err(|why| why.to_error(GET_DIFF_PREVIEW, &self.cwd))
    }

    fn preview(&self) -> Result<Value, Unavailable> {
        let root = WorkspaceRoot::check(&self.cwd).map_err(|why| Unavailable::Unusable {
            detail: why.message(),
        })?;
        match git::text(root.path(), &["rev-parse", "--is-inside-work-tree"]) {
            Ok(inside) if inside.trim() == "true" => {}
            Ok(_) => return Ok(result(root.display(), vec![])),
            Err(why) if git::is_not_a_repository(&why) => {
                return Ok(result(root.display(), vec![]));
            }
            Err(why) => return Err(why),
        }

        let ignore = self.ignore_whitespace.unwrap_or(false);
        let working = checkpoints::working_tree_patch(root.path(), ignore)?;
        let branch = git::text(root.path(), &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .ok()
            .map(|name| name.trim().to_string());
        let has_head = checkpoints::present(root.path(), "HEAD");
        let base = self.base_ref.clone().or_else(|| {
            if !has_head {
                return None;
            }
            let remote = git::primary_remote(root.path());
            let default = git::default_ref(root.path(), remote.as_deref())?;
            // The default may exist only as a remote-tracking ref. Prefer that
            // observation when available rather than a stale local checkout.
            if let Some(remote) = remote {
                let remote_ref = format!("{remote}/{default}");
                if checkpoints::present(root.path(), &remote_ref) {
                    return Some(remote_ref);
                }
            }
            checkpoints::present(root.path(), &default).then_some(default)
        });
        let branch_patch = if let Some(base) = base.as_deref().filter(|_| has_head) {
            // Resolve client text to an object before passing it to diff. The
            // merge base gives branch-only changes even when the base advanced.
            let revision = git::text(
                root.path(),
                &[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    &format!("{base}^{{commit}}"),
                ],
            )?;
            let merge_base = git::text(root.path(), &["merge-base", revision.trim(), "HEAD"])?;
            checkpoints::patch_trees(root.path(), merge_base.trim(), "HEAD", ignore, true)?
        } else {
            Patch::default()
        };
        Ok(result(
            root.display(),
            vec![
                source("working-tree", "Working tree", Some("HEAD"), None, working),
                source(
                    "branch-range",
                    &base
                        .as_ref()
                        .map(|base| format!("Against {base}"))
                        .unwrap_or_else(|| "Against base branch".to_string()),
                    base.as_deref(),
                    Some(branch.as_deref().unwrap_or("HEAD")),
                    branch_patch,
                ),
            ],
        ))
    }
}

fn result(cwd: &str, sources: Vec<Value>) -> Value {
    json!({"cwd": cwd, "generatedAt": crate::clock::now_iso(), "sources": sources})
}

fn source(kind: &str, title: &str, base: Option<&str>, head: Option<&str>, patch: Patch) -> Value {
    json!({
        "id": kind,
        "kind": kind,
        "title": title,
        "baseRef": base,
        "headRef": head,
        "diffHash": format!("{:x}", Sha256::digest(patch.diff.as_bytes())),
        "diff": patch.diff,
        "truncated": patch.truncated,
    })
}

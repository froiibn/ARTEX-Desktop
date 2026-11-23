//! Task-owned report projection. Rendering/export never starts another assessment.
use super::*;

#[derive(Default)]
pub(super) struct SiteReportState {
    owner: Option<(String, String, String)>,
    read_seq: u64,
    pending: bool,
    error: Option<String>,
    settlement: Option<(String, usize, usize, usize)>,
    page_request: Option<(String, String, u64)>,
    page: Option<Value>,
    page_offset: u64,
    previous_pages: Vec<u64>,
    page_error: Option<String>,
}
impl SiteReportState {
    pub(super) fn received(&mut self) {
        self.pending = false;
        self.error = None;
    }
}
impl Panes {
    pub(super) fn retire_site_report(&mut self) {
        self.site_report = SiteReportState::default();
        self.projection.remove("ui.report");
        for method in ["ui.report", "native.report.page"] {
            if let Some(token) = self.flow.latest.remove(method) {
                self.flow.append.remove(&token);
            }
            self.flow.versions.remove(method);
        }
        self.flow
            .bodies
            .retain(|_, body| body.method != "ui.report");
    }

    /// Use the service's immutable read generation, not a guessed UI receipt time.
    pub(crate) fn site_report_read_seq(&mut self, token: &str, read_seq: u64) {
        if self
            .flow
            .latest
            .get("ui.report")
            .is_some_and(|t| t == token)
        {
            self.site_report.read_seq = read_seq;
        }
    }

    pub(super) fn site_report_error(
        &mut self,
        method: &str,
        token: &str,
        code: &str,
        message: &str,
    ) -> bool {
        if method == "native.report.page" {
            if self.report_page_is_current(token) {
                self.site_report.page_request = None;
                self.site_report.page_error = Some(format!("{code}: {message}"));
                self.flow.append.remove(token);
                return true;
            }
            return false;
        }
        let current = if method == "ui.report" {
            self.flow.latest.get(method).is_some_and(|t| t == token)
        } else if method == "native.body.read" {
            self.flow.bodies.get(token).is_some_and(|b| {
                b.method == "ui.report"
                    && self.flow.latest.get("ui.report") == b.parent_token.as_ref()
            })
        } else {
            false
        };
        if current {
            self.flow.bodies.remove(token);
            self.site_report.pending = false;
            self.site_report.error = Some(format!("{code}: {message}"));
        }
        current
    }

    fn request_site_report(
        &mut self,
        snapshot: &Snapshot,
        task: &Record,
        effects: &mut Vec<Effect>,
    ) {
        // Retire both the old text and hydration token before the next read.
        // Export must always use the same task and bytes currently on screen.
        self.retire_site_report();
        self.site_report.owner = Some((
            snapshot.workspace_id.clone(),
            self.engine_epoch.clone(),
            task.header.id.clone(),
        ));
        self.site_report.read_seq = snapshot.commit_seq;
        self.site_report.pending = true;
        self.site_report.settlement = Some(settlement(snapshot, task));
        self.request(effects, "ui.report", json!({"task":task.header.id}), false);
    }

    fn report_page_is_current(&self, token: &str) -> bool {
        self.site_report
            .page_request
            .as_ref()
            .is_some_and(|(request, parent, _)| {
                request == token && self.flow.latest.get("ui.report") == Some(parent)
            })
    }
    pub(super) fn receive_report_page(&mut self, token: &str, value: &Value) -> bool {
        if !self.report_page_is_current(token) {
            return false;
        }
        self.flow.append.remove(token);
        let offset = self.site_report.page_request.take().unwrap().2;
        let total = value["total_length"].as_str().and_then(decimal);
        let source_total = self
            .projection
            .get("ui.report")
            .and_then(|b| b["blob"]["length"].as_str())
            .and_then(decimal);
        let valid = value["text"].as_str().is_some_and(|text| {
            let Some(total) = total.filter(|t| Some(*t) == source_total) else {
                return false;
            };
            let Some(end) = offset.checked_add(text.len() as u64) else {
                return false;
            };
            text.len() <= artex_native_core::NATIVE_BODY_PAGE_BYTES
                && offset <= total
                && end <= total
                && if value.get("next_offset").is_some_and(Value::is_null) {
                    end == total
                } else {
                    value["next_offset"].as_str().and_then(decimal) == Some(end)
                        && end > offset
                        && end < total
                }
        });
        if !valid {
            self.site_report.page = None;
            self.site_report.page_error = Some(
                "EXTERNAL_FAILURE: 보고서 페이지 응답이 내부 계약과 일치하지 않습니다.".into(),
            );
            return true;
        }
        self.site_report.page_offset = offset;
        self.site_report.page = Some(value.clone());
        self.site_report.page_error = None;
        true
    }
    fn request_report_page(&mut self, offset: u64, effects: &mut Vec<Effect>) {
        let Some(body) = self.projection.get("ui.report").cloned() else {
            return;
        };
        let Some(parent) = self.flow.latest.get("ui.report").cloned() else {
            return;
        };
        self.site_report.page = None;
        self.site_report.page_error = None;
        self.request(
            effects,
            "native.report.page",
            json!({"body":body,"offset":offset}),
            false,
        );
        self.site_report.page_request = Some((
            self.flow.latest["native.report.page"].clone(),
            parent,
            offset,
        ));
    }

    pub(super) fn show_site_report(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        task: &Record,
        effects: &mut Vec<Effect>,
    ) {
        let owner = (
            snapshot.workspace_id.clone(),
            self.engine_epoch.clone(),
            task.header.id.clone(),
        );
        if self.site_report.owner.as_ref() != Some(&owner)
            || (!self.site_report.pending
                && self.site_report.error.is_none()
                && self.site_report.settlement.as_ref() != Some(&settlement(snapshot, task)))
        {
            self.request_site_report(snapshot, task, effects);
        }
        ui.heading("사이트 종합 결과 보고서");
        ui.label("이 작업에 저장된 검사·발견·증거를 종합합니다. 다른 사이트를 새로 검사하거나 모델을 호출하지 않습니다.");
        ui.small(format!(
            "작업 {} · 저장 세대 {}",
            task.header.id, self.site_report.read_seq
        ));
        if snapshot.commit_seq > self.site_report.read_seq {
            ui.small("작업 공간에 새 저장 기록이 있습니다. 현재 표시본은 위 세대 기준이며 다시 불러오면 갱신됩니다.");
        }

        // Large evidence is kept once in the projection; copying megabytes on
        // every repaint would make an otherwise read-only report stall the UI.
        let ready = self
            .projection
            .get("ui.report")
            .is_some_and(|b| b["text"].is_string() || b["blob"].is_object())
            && !self.site_report.pending
            && self.site_report.error.is_none();
        let (mut reload, mut export, mut copy) = (false, false, false);
        crate::theme::toolbar(ui, |ui| {
            reload = ui
                .add_enabled(
                    !self.site_report.pending,
                    egui::Button::new("보고서 다시 불러오기"),
                )
                .clicked();
            export = ui
                .add_enabled(ready, egui::Button::new("보고서 내보내기 (.md)"))
                .clicked();
            let copy_ready = ready
                && (self.projection["ui.report"]["text"].is_string()
                    || self.site_report.page.is_some());
            let copy_label = if self
                .projection
                .get("ui.report")
                .is_some_and(|b| b["blob"].is_object())
            {
                "현재 페이지 복사"
            } else {
                "보고서 복사"
            };
            copy = ui
                .add_enabled(copy_ready, egui::Button::new(copy_label))
                .clicked();
        });
        if reload {
            self.request_site_report(snapshot, task, effects);
        } else {
            if export {
                if let Some(body) = self.projection.get("ui.report") {
                    // Preserve the entire verified Body, including all pages.
                    effects.push(Effect::ExportBody {
                        name: format!("site-assessment-{}.md", task.header.id),
                        body: body.clone(),
                    });
                }
            }
            if copy {
                if let Some(text) = self
                    .projection
                    .get("ui.report")
                    .and_then(|b| b["text"].as_str())
                    .or_else(|| {
                        self.site_report
                            .page
                            .as_ref()
                            .and_then(|p| p["text"].as_str())
                    })
                {
                    ui.ctx().copy_text(text.to_owned());
                }
            }
        }
        if self.site_report.pending {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("저장된 분석 기록으로 보고서를 구성하고 있습니다.");
            });
            return;
        }
        if let Some(error) = &self.site_report.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            ui.label("기록을 읽지 못한 상태에서 빈 보고서나 분석 성공으로 표시하지 않습니다. 다시 불러오기로 재시도할 수 있습니다.");
            return;
        }
        if self
            .projection
            .get("ui.report")
            .is_some_and(|b| b["blob"].is_object())
        {
            // Page reads are bounded even when the complete report exceeds the
            // ordinary full-text reader's limit. They retain the original Body.
            if self.site_report.page.is_none()
                && self.site_report.page_request.is_none()
                && self.site_report.page_error.is_none()
            {
                self.request_report_page(0, effects);
            }
            if let Some(error) = &self.site_report.page_error {
                ui.colored_label(ui.visuals().error_fg_color, error);
                ui.label("본문 조회에 실패했습니다. 다시 불러오기로 재시도할 수 있습니다. 전체 파일 저장은 원본을 별도로 검증합니다.");
            } else if let Some(page) = &self.site_report.page {
                let current = self.site_report.page_offset;
                let next = page["next_offset"]
                    .as_str()
                    .and_then(|v| v.parse::<u64>().ok());
                let text = page["text"].as_str().unwrap_or("");
                ui.small(format!(
                    "본문 페이지 · {}–{} / {} 바이트 · 내보내기는 모든 페이지를 포함합니다.",
                    current,
                    current + text.len() as u64,
                    page["total_length"].as_str().unwrap_or("?")
                ));
                let (mut previous, mut advance) = (false, false);
                crate::theme::toolbar(ui, |ui| {
                    previous = ui
                        .add_enabled(
                            !self.site_report.previous_pages.is_empty(),
                            egui::Button::new("이전 페이지"),
                        )
                        .clicked();
                    advance = ui
                        .add_enabled(next.is_some(), egui::Button::new("다음 페이지"))
                        .clicked();
                });
                // A page may begin inside a fence. Render it verbatim rather
                // than interpreting partial evidence as authoritative headings.
                ui.add(
                    egui::Label::new(RichText::new(text).monospace())
                        .selectable(true)
                        .wrap(),
                );
                if previous {
                    let offset = self.site_report.previous_pages.pop().unwrap();
                    self.request_report_page(offset, effects);
                } else if advance {
                    self.site_report.previous_pages.push(current);
                    self.request_report_page(next.unwrap(), effects);
                }
            } else {
                ui.label("보고서 페이지를 불러오고 있습니다.");
            }
            return;
        }
        if let Some(report) = self
            .projection
            .get("ui.report")
            .and_then(|b| b["text"].as_str())
        {
            ui.separator();
            show_markdown(ui, report);
        }
    }
}

fn decimal(value: &str) -> Option<u64> {
    value.parse::<u64>().ok().filter(|n| n.to_string() == value)
}

// Completion transitions refresh automatically, while unrelated writes leave
// the displayed generation stable so readers can finish paging or exporting it.
fn settlement(s: &Snapshot, task: &Record) -> (String, usize, usize, usize) {
    let runs = s
        .records("agent_runs")
        .into_iter()
        .filter(|r| r.data["task_id"] == task.header.id)
        .collect::<Vec<_>>();
    let ids = runs
        .iter()
        .map(|r| r.header.id.as_str())
        .collect::<BTreeSet<_>>();
    let active = runs
        .iter()
        .filter(|r| matches!(text(&r.data, "state"), "queued" | "running" | "waiting"))
        .count();
    let pending = s
        .records("tool_invocations")
        .into_iter()
        .filter(|r| {
            ids.contains(text(&r.data, "run_id"))
                && !matches!(text(&r.data, "state"), "completed" | "failed" | "cancelled")
        })
        .count();
    let recovery = runs
        .iter()
        .filter(|r| matches!(text(&r.data, "state"), "interrupted" | "recovery_required"))
        .count()
        + s.records("task_runtime")
            .into_iter()
            .filter(|r| {
                r.data["task_id"] == task.header.id
                    && r.data["accounting_state"] == "recovery_required"
            })
            .count();
    (task.header.version.clone(), active, pending, recovery)
}

/// A read-only native presentation of headings, lists and fenced evidence.
/// No HTML, remote images or clickable target content is executed by this view.
fn show_markdown(ui: &mut egui::Ui, report: &str) {
    let mut fence: Option<(char, usize)> = None;
    for line in report.lines() {
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'));
        if let Some((kind, width)) = fence {
            let count = trimmed.chars().take_while(|c| *c == kind).count();
            if count >= width && trimmed[count..].trim().is_empty() {
                fence = None;
                ui.add_space(6.0);
            } else {
                ui.add(
                    egui::Label::new(RichText::new(line).monospace())
                        .selectable(true)
                        .wrap(),
                );
            }
        } else if let Some(kind) =
            marker.filter(|c| trimmed.chars().take_while(|x| x == c).count() >= 3)
        {
            fence = Some((kind, trimmed.chars().take_while(|c| *c == kind).count()));
            ui.add_space(6.0);
        } else if let Some(title) = line.strip_prefix("# ") {
            ui.heading(title);
        } else if let Some(title) = line.strip_prefix("## ") {
            ui.add_space(10.0);
            ui.separator();
            ui.heading(title);
        } else if let Some(title) = line.strip_prefix("### ") {
            ui.add_space(6.0);
            ui.strong(title);
        } else if line.is_empty() {
            ui.add_space(4.0);
        } else {
            ui.add(egui::Label::new(line).selectable(true).wrap());
        }
    }
}

#[cfg(test)]
#[path = "site_report_tests.rs"]
mod tests;

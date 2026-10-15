//! Dashboard presentation uses the same snapshot, filters and selection routes.
use super::{
    Effect, TranscriptPart, active_run, compact_time, live_task, pending_record,
    presentation_short_time, recent_first, run_row, state_label, transcript_parts,
    visible_activity,
};
use crate::{
    model::{Section, display_name, text},
    theme::{self, BadgeTone, Icon},
};
use artex_core::{Record, Snapshot};
use eframe::egui::{self, RichText};

pub(super) fn show(ui: &mut egui::Ui, snapshot: &Snapshot, effects: &mut Vec<Effect>) {
    let tasks = snapshot
        .records("tasks")
        .into_iter()
        .filter(|r| live_task(r))
        .count();
    let runs = snapshot
        .records("agent_runs")
        .into_iter()
        .filter(|r| active_run(r))
        .count();
    let approvals = snapshot
        .records("intercept_pending")
        .into_iter()
        .chain(snapshot.records("user_questions"))
        .filter(|r| pending_record(r))
        .count();
    let p = theme::palette(ui.visuals().dark_mode);
    ui.horizontal_wrapped(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("작업 공간 현황").size(26.0).strong());
            ui.weak("진행 중인 작업과 확인할 요청을 한눈에 살펴보세요.");
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let color = if approvals == 0 { p.muted } else { p.warning };
            ui.colored_label(
                color,
                RichText::new(if approvals == 0 {
                    "확인할 요청 없음".into()
                } else {
                    format!("확인 대기 {approvals}건")
                })
                .small(),
            );
            let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(10.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 3.0, color);
        });
    });
    ui.add_space(16.0);
    let statistics = [
        ("작업", tasks, Icon::Tasks, p.accent),
        ("실행 중", runs, Icon::Agents, p.success),
        ("확인 대기", approvals, Icon::Tools, p.warning),
        (
            "발견 사항",
            snapshot.records("findings").len(),
            Icon::Findings,
            p.warning,
        ),
        (
            "자산",
            snapshot.records("assets").len(),
            Icon::Workspace,
            p.accent,
        ),
    ];
    let columns = if ui.available_width() >= 650.0 {
        5
    } else if ui.available_width() >= 510.0 {
        3
    } else {
        2
    };
    for row in statistics.chunks(columns) {
        ui.columns(columns, |uis| {
            for (index, (label, count, icon, color)) in row.iter().enumerate() {
                let ui = &mut uis[index];
                let width = ui.available_width();
                panel(ui).inner_margin(12).show(ui, |ui| {
                    ui.set_width((width - 24.0).max(1.0));
                    ui.horizontal(|ui| {
                        symbol(ui, *icon, *color, 32.0);
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Label::new(RichText::new(*label).small().color(p.muted))
                                    .truncate(),
                            );
                            ui.label(RichText::new(count.to_string()).size(28.0).strong());
                        });
                    });
                });
            }
        });
        ui.add_space(8.0);
    }
    ui.add_space(8.0);
    if ui.available_width() >= 760.0 {
        ui.columns(2, |uis| {
            tasks_panel(&mut uis[0], snapshot, effects);
            approvals_panel(&mut uis[1], snapshot, effects);
        });
    } else {
        tasks_panel(ui, snapshot, effects);
        ui.add_space(14.0);
        approvals_panel(ui, snapshot, effects);
    }
    ui.add_space(16.0);
    let width = ui.available_width();
    panel(ui).show(ui, |ui| {
        ui.set_width((width - 32.0).max(1.0));
        heading(ui, Icon::Logs, "최근 활동");
        let mut entries = snapshot.records("activity");
        entries.extend(snapshot.records("conversation_activities"));
        entries.retain(|r| visible_activity(text(&r.data, "kind")));
        recent_first(&mut entries);
        if entries.is_empty() {
            empty(
                ui,
                Icon::Logs,
                "아직 기록된 활동이 없습니다.",
                "작업을 시작하면 실행과 변경 기록이 여기에 표시됩니다.",
            );
        }
        let count = entries.len().min(8);
        for (index, entry) in entries.into_iter().take(count).enumerate() {
            ui.push_id(&entry.header.id, |ui| {
                let height = 50.0;
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(18.0, height), egui::Sense::hover());
                    let center = egui::pos2(rect.center().x, rect.top() + 10.0);
                    ui.painter().circle_filled(center, 4.0, p.accent);
                    if index + 1 < count {
                        ui.painter().line_segment(
                            [
                                center + egui::vec2(0.0, 8.0),
                                egui::pos2(center.x, rect.bottom() + 9.0),
                            ],
                            egui::Stroke::new(1.0_f32, p.border),
                        );
                    }
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());
                        let title = activity_title(entry);
                        ui.add(egui::Label::new(RichText::new(&title).strong()).truncate())
                            .on_hover_text(title);
                        ui.horizontal_wrapped(|ui| {
                            ui.small(presentation_short_time(&entry.header.created_at))
                                .on_hover_text(compact_time(&entry.header.created_at));
                            let worker = text(&entry.data, "worker");
                            ui.weak(match worker {
                                "" => "시스템",
                                "auto" => "자동 실행",
                                other => other,
                            });
                        });
                    });
                });
            });
        }
    });
    ui.add_space(14.0);
    let mut running: Vec<_> = snapshot
        .records("agent_runs")
        .into_iter()
        .filter(|r| active_run(r))
        .collect();
    recent_first(&mut running);
    egui::CollapsingHeader::new(format!("실행 중인 에이전트 · {}", running.len()))
        .id_salt("dashboard-runs")
        .show(ui, |ui| {
            if running.is_empty() {
                ui.weak("현재 실행 중인 에이전트가 없습니다.");
            }
            for run in running.into_iter().take(20) {
                run_row(ui, snapshot, run, effects);
            }
        });
}

fn panel(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::new()
        .fill(theme::palette(ui.visuals().dark_mode).panel)
        .corner_radius(12)
        .inner_margin(16)
}

fn heading(ui: &mut egui::Ui, icon: Icon, title: &str) {
    let p = theme::palette(ui.visuals().dark_mode);
    ui.horizontal(|ui| {
        symbol(ui, icon, p.accent, 24.0);
        ui.label(RichText::new(title).size(17.0).strong());
    });
    ui.add_space(12.0);
}

fn symbol(ui: &mut egui::Ui, icon: Icon, color: egui::Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), size * 0.5, color.gamma_multiply(0.16));
    theme::paint_icon(ui.painter(), rect.shrink(6.0), icon, color);
}

fn empty(ui: &mut egui::Ui, icon: Icon, title: &str, detail: &str) {
    let p = theme::palette(ui.visuals().dark_mode);
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        symbol(ui, icon, p.muted, 38.0);
        ui.vertical(|ui| {
            ui.label(title);
            ui.weak(detail);
        });
    });
    ui.add_space(10.0);
}

fn state(ui: &mut egui::Ui, status: &str) {
    let tone = match status {
        "running" | "queued" => BadgeTone::Info,
        "completed" | "succeeded" | "done" => BadgeTone::Success,
        "failed" | "blocked" => BadgeTone::Error,
        "paused" | "pending" | "waiting" | "interrupted" => BadgeTone::Warning,
        _ => BadgeTone::Neutral,
    };
    ui.horizontal(|ui| {
        theme::badge(ui, state_label(status), tone);
    });
}

fn tasks_panel(ui: &mut egui::Ui, snapshot: &Snapshot, effects: &mut Vec<Effect>) {
    let width = ui.available_width();
    panel(ui).show(ui, |ui| {
        ui.set_width((width - 32.0).max(1.0));
        heading(ui, Icon::Tasks, "최근 작업");
        let mut tasks: Vec<_> = snapshot
            .records("tasks")
            .into_iter()
            .filter(|r| live_task(r))
            .collect();
        recent_first(&mut tasks);
        if tasks.is_empty() {
            empty(
                ui,
                Icon::Tasks,
                "아직 등록된 작업이 없습니다.",
                "왼쪽 ‘작업’ 메뉴에서 새 분석 작업을 만들 수 있습니다.",
            );
        }
        for task in tasks.into_iter().take(6) {
            ui.push_id(&task.header.id, |ui| {
                let p = theme::palette(ui.visuals().dark_mode);
                theme::surface_frame(ui.style()).show(ui, |ui| {
                    ui.set_width((width - 64.0).max(1.0));
                    state(ui, text(&task.data, "status"));
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(display_name(task)).strong().color(p.accent),
                            )
                            .frame(false)
                            .wrap(),
                        )
                        .on_hover_text("이 작업 열기")
                        .clicked()
                    {
                        effects.push(Effect::Select {
                            section: Section::Tasks,
                            id: task.header.id.clone(),
                        });
                    }
                    let exploration = text(&task.data, "exploration_id");
                    let runs = snapshot
                        .records("agent_runs")
                        .into_iter()
                        .filter(|r| r.data["task_id"] == task.header.id && active_run(r))
                        .count();
                    let findings = snapshot
                        .records("findings")
                        .into_iter()
                        .filter(|r| {
                            r.data["task_id"] == task.header.id
                                || r.data["exploration_id"] == exploration
                        })
                        .count();
                    ui.small(format!("실행 중 {runs} · 발견 사항 {findings}"));
                    ui.weak(presentation_short_time(&task.header.updated_at))
                        .on_hover_text(compact_time(&task.header.updated_at));
                });
                ui.add_space(8.0);
            });
        }
    });
}

fn approvals_panel(ui: &mut egui::Ui, snapshot: &Snapshot, effects: &mut Vec<Effect>) {
    let width = ui.available_width();
    panel(ui).show(ui, |ui| {
        ui.set_width((width - 32.0).max(1.0));
        heading(ui, Icon::Tools, "승인 요청 · 사용자 질문");
        let mut pending = snapshot.records("intercept_pending");
        pending.extend(snapshot.records("user_questions"));
        pending.retain(|r| pending_record(r));
        recent_first(&mut pending);
        if pending.is_empty() {
            empty(
                ui,
                Icon::Tools,
                "현재 확인할 요청이 없습니다.",
                "승인이나 답변이 필요하면 이곳에 표시됩니다.",
            );
        }
        for item in pending.into_iter().take(6) {
            ui.push_id(&item.header.id, |ui| {
                let question = item.header.collection == "user_questions";
                theme::surface_frame(ui.style()).show(ui, |ui| {
                    ui.set_width((width - 64.0).max(1.0));
                    state(ui, "pending");
                    ui.strong(if question {
                        "사용자 질문"
                    } else {
                        "도구 실행 승인"
                    });
                    let description = if question {
                        item.data["payload"]["questions"][0]["question"]
                            .as_str()
                            .unwrap_or(text(&item.data, "kind"))
                    } else {
                        text(&item.data, "tool_name")
                    };
                    ui.add(
                        egui::Label::new(description.chars().take(120).collect::<String>()).wrap(),
                    );
                    if theme::primary_button(ui, "요청 확인").clicked() {
                        effects.push(Effect::Select {
                            section: if question {
                                Section::Sessions
                            } else {
                                Section::Approvals
                            },
                            id: if question {
                                text(&item.data, "run_id").into()
                            } else {
                                item.header.id.clone()
                            },
                        });
                    }
                    ui.small(presentation_short_time(&item.header.created_at))
                        .on_hover_text(compact_time(&item.header.created_at));
                });
                ui.add_space(8.0);
            });
        }
    });
}

fn activity_title(entry: &Record) -> String {
    transcript_parts(&entry.data)
        .into_iter()
        .find_map(|part| match part {
            TranscriptPart::Text(text) => Some(text),
            TranscriptPart::Thinking(_) => Some("모델 추론 기록".into()),
            TranscriptPart::Audit { title, .. } => Some(title),
            TranscriptPart::Tool { name, result, .. } => Some(format!(
                "{} · {name}",
                if result {
                    "도구 실행 결과"
                } else {
                    "도구 실행 요청"
                }
            )),
        })
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| display_name(entry))
}

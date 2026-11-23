//! Source task-creation options, kept as a draft until the user admits creation.
use super::{Effect, Panes, command, display_name, multiline, text};
use artex_core::{Record, Snapshot};
use eframe::egui::{self, Ui};
use serde_json::{Value, json};
// Source create-task controls and task planner share this ten-minute minimum.
const MIN_PLAN_HEARTBEAT_SECONDS: i32 = 600;

pub(super) struct TaskCreation {
    category: Option<String>,
    companies: Vec<String>,
    timeout: String,
    heartbeat: String,
    seed_first_intent: bool,
    coverage: bool,
    rules: Vec<Value>,
    template: Option<String>,
    pending_template: Option<Record>,
}
impl Default for TaskCreation {
    fn default() -> Self {
        Self {
            category: None,
            companies: vec![],
            timeout: "0".into(),
            // Source task creation normalizes 0 to the ten-minute default.
            heartbeat: "0".into(),
            seed_first_intent: false,
            coverage: true,
            rules: vec![],
            template: None,
            pending_template: None,
        }
    }
}
fn unavailable_selections(
    ui: &mut Ui,
    selected: &mut Vec<String>,
    label: &str,
    unavailable: impl Fn(&str) -> bool,
) {
    // Selected references can disappear while this draft survives navigation.
    // Show their exact identity until the user explicitly removes that choice.
    let mut remove = None;
    for id in selected.iter().filter(|id| unavailable(id)) {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("사용할 수 없는 {label}: {id}"));
            if ui.button(format!("{label} 선택 해제: {id}")).clicked() {
                remove = Some(id.clone());
            }
        });
    }
    if let Some(id) = remove {
        selected.retain(|selected| selected != &id);
    }
}
impl Panes {
    fn apply_task_template(&mut self, template: Record) {
        // A template copies values; it does not remain bound to the new task.
        self.description = text(&template.data, "description").into();
        self.goal = text(&template.data, "goal").into();
        self.creation.category = template.data["category_id"].as_str().map(str::to_owned);
        self.creation.rules = template.data["intercept_rules"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        self.creation.template = Some(template.header.id);
        self.creation.pending_template = None;
    }
    fn task_creation_input(&self, snapshot: &Snapshot) -> Result<Value, String> {
        if self.description.trim().is_empty() || self.goal.trim().is_empty() {
            return Err("설명과 목표를 입력하세요.".into());
        }
        let seconds = |value: &str| value.trim().parse::<i32>().ok().filter(|v| *v >= 0);
        let timeout =
            seconds(&self.creation.timeout).ok_or("제한 시간은 0 이상의 정수 초로 입력하세요.")?;
        let heartbeat = seconds(&self.creation.heartbeat)
            .ok_or("계획 주기는 0 이상의 정수 초로 입력하세요.")?;
        if heartbeat != 0 && heartbeat < MIN_PLAN_HEARTBEAT_SECONDS {
            return Err("계획 주기는 0(기본값) 또는 600초 이상으로 입력하세요.".into());
        }
        if self
            .creation
            .category
            .as_deref()
            .is_some_and(|id| snapshot.get("task_categories", id).is_none())
        {
            return Err("선택한 분류가 삭제되었습니다. 분류를 다시 선택하세요.".into());
        }
        if self.creation.companies.len() > 32
            || self
                .creation
                .companies
                .iter()
                .any(|id| snapshot.get("companies", id).is_none())
        {
            return Err("현재 존재하는 회사를 최대 32개까지 선택하세요.".into());
        }
        if self
            .selected_profiles
            .iter()
            .any(|id| snapshot.get("llm_profiles", id).is_none())
        {
            return Err("사용할 수 없는 모델 선택을 해제하세요.".into());
        }
        if self.selected_sources.iter().any(|id| {
            snapshot.get("tasks", id).is_none_or(|task| {
                !task.data["deleted_at"].is_null() || !task.data["archived_at"].is_null()
            })
        }) {
            return Err("삭제되거나 보관된 원본 작업 선택을 해제하세요.".into());
        }
        let rules = self
            .creation
            .rules
            .iter()
            .filter(|r| !text(r, "pattern").trim().is_empty())
            .cloned()
            .collect::<Vec<_>>();
        Ok(
            json!({"input":{"name":self.title.trim(),"description":self.description.trim(),"goal":self.goal.trim(),"llmProfileIds":self.selected_profiles,"sourceTaskIds":self.selected_sources,"categoryId":self.creation.category,"companyIds":self.creation.companies,"timeoutSeconds":timeout,"seedFirstIntent":self.creation.seed_first_intent,"planHeartbeatSeconds":heartbeat,"coverageEnabled":self.creation.coverage,"interceptRules":rules}}),
        )
    }
    pub(super) fn new_task(&mut self, ui: &mut Ui, snapshot: &Snapshot, effects: &mut Vec<Effect>) {
        ui.heading("새 작업");
        let mut chosen = None;
        crate::theme::field_row(ui, "작업 템플릿", |ui| {
            let label = self
                .creation
                .template
                .as_deref()
                .and_then(|id| snapshot.get("task_templates", id))
                .map(display_name)
                .unwrap_or_else(|| "사용하지 않음".into());
            egui::ComboBox::from_id_salt("new-task-template")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(self.creation.template.is_none(), "사용하지 않음")
                        .clicked()
                    {
                        self.creation.template = None;
                    }
                    for template in snapshot.records("task_templates") {
                        if ui
                            .selectable_label(
                                self.creation.template.as_deref() == Some(&template.header.id),
                                display_name(template),
                            )
                            .clicked()
                        {
                            chosen = Some(template.clone());
                        }
                    }
                });
        });
        if let Some(template) = chosen {
            if self.description.trim().is_empty()
                && self.goal.trim().is_empty()
                && self.creation.rules.is_empty()
            {
                self.apply_task_template(template);
            } else {
                self.creation.pending_template = Some(template);
            }
        }
        if let Some(template) = self.creation.pending_template.clone() {
            ui.group(|ui| {
                ui.label(format!(
                    "‘{}’의 설명·목표·분류·차단 규칙으로 현재 초안을 교체할까요?",
                    display_name(&template)
                ));
                ui.horizontal_wrapped(|ui| {
                    if ui.button("템플릿 적용").clicked() {
                        self.apply_task_template(template.clone());
                    }
                    if ui.button("현재 초안 유지").clicked() {
                        self.creation.pending_template = None;
                    }
                });
            });
        }
        crate::theme::field_row(ui, "이름", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.title).desired_width(f32::INFINITY));
        });
        multiline(ui, "설명", &mut self.description, 3);
        multiline(ui, "목표와 허용 범위", &mut self.goal, 4);
        crate::theme::field_row(ui, "분류", |ui| {
            let label = self
                .creation
                .category
                .as_deref()
                .and_then(|id| snapshot.get("task_categories", id))
                .map(display_name)
                .unwrap_or_else(|| "분류 없음".into());
            egui::ComboBox::from_id_salt("new-task-category")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.creation.category, None, "분류 없음");
                    for category in snapshot.records("task_categories") {
                        ui.selectable_value(
                            &mut self.creation.category,
                            Some(category.header.id.clone()),
                            display_name(category),
                        );
                    }
                });
        });
        ui.strong("모델 실행 순서");
        unavailable_selections(ui, &mut self.selected_profiles, "모델", |id| {
            snapshot.get("llm_profiles", id).is_none()
        });
        for profile in snapshot.records("llm_profiles") {
            selection(ui, &mut self.selected_profiles, profile);
        }
        ui.collapsing("정보를 공유할 직접 원본 작업", |ui| {
            unavailable_selections(ui, &mut self.selected_sources, "원본 작업", |id| {
                snapshot.get("tasks", id).is_none_or(|task| {
                    !task.data["deleted_at"].is_null() || !task.data["archived_at"].is_null()
                })
            });
            for source in snapshot
                .records("tasks")
                .into_iter()
                .filter(|r| r.data["deleted_at"].is_null() && r.data["archived_at"].is_null())
            {
                let checked = self.selected_sources.contains(&source.header.id);
                ui.add_enabled_ui(checked || self.selected_sources.len() < 8, |ui| {
                    selection(ui, &mut self.selected_sources, source);
                });
            }
        });
        ui.collapsing("연결할 회사", |ui| {
            ui.weak("선택한 회사의 범위와 자산을 작업에 연결합니다.");
            // Keep stale selections visible until the user explicitly removes
            // them: navigation/deletion must not silently discard a creation draft.
            let mut remove = None;
            for id in &self.creation.companies {
                if snapshot.get("companies", id).is_none() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("삭제되었거나 없는 회사: {id}"));
                        if ui.button(format!("회사 선택 해제: {id}")).clicked() {
                            remove = Some(id.clone());
                        }
                    });
                }
            }
            if let Some(id) = remove {
                self.creation.companies.retain(|selected| selected != &id);
            }
            for company in snapshot.records("companies") {
                let checked = self.creation.companies.contains(&company.header.id);
                ui.add_enabled_ui(checked || self.creation.companies.len() < 32, |ui| {
                    selection(ui, &mut self.creation.companies, company);
                });
            }
        });
        ui.collapsing("실행 옵션", |ui| {
            crate::theme::field_row(ui, "제한 시간 (초, 0 = 무제한)", |ui| {
                ui.text_edit_singleline(&mut self.creation.timeout);
            });
            crate::theme::field_row(ui, "계획 주기 (초, 0 = 기본값)", |ui| {
                ui.text_edit_singleline(&mut self.creation.heartbeat);
            });
            ui.checkbox(
                &mut self.creation.seed_first_intent,
                "생성할 때 최초 의도를 바로 준비",
            );
            ui.checkbox(&mut self.creation.coverage, "자산 커버리지 기능 사용");
        });
        ui.collapsing("작업 전용 자산 차단·허용 규칙", |ui| {
            let mut remove = None;
            for (i, rule) in self.creation.rules.iter_mut().enumerate() {
                ui.push_id(i, |ui| { ui.group(|ui| {
                    let mut enabled = rule["enabled"].as_bool().unwrap_or(false);
                    ui.checkbox(&mut enabled, "규칙 사용"); rule["enabled"] = json!(enabled);
                    choice(ui, rule, "action", "동작", &["block", "allow"]);
                    choice(ui, rule, "kind", "일치 방식", &["exact_domain","exact_ip","exact_url","cidr","fuzzy_domain","fuzzy_ip","fuzzy_url"]);
                    for (field, label) in [("pattern", "대상 패턴"), ("note", "메모")] {
                        let mut value = text(rule, field).to_owned();
                        crate::theme::field_row(ui, label, |ui| { ui.text_edit_singleline(&mut value); });
                        rule[field] = json!(value);
                    }
                    if ui.button("규칙 삭제").clicked() { remove = Some(i); }
                }); });
            }
            if let Some(i) = remove { self.creation.rules.remove(i); }
            if ui.button("규칙 추가").clicked() { self.creation.rules.push(json!({"enabled":true,"action":"block","kind":"exact_domain","pattern":"","note":""})); }
        });
        let input = self.task_creation_input(snapshot);
        if let Err(error) = &input {
            ui.weak(error);
        }
        if ui
            .add_enabled(
                !self.read_only && input.is_ok() && self.creation.pending_template.is_none(),
                egui::Button::new("작업 만들기"),
            )
            .clicked()
        {
            command(effects, "ui.createTask", input.unwrap(), vec![]);
        }
    }
}
fn selection(ui: &mut Ui, ids: &mut Vec<String>, row: &Record) {
    let mut checked = ids.contains(&row.header.id);
    if ui.checkbox(&mut checked, display_name(row)).changed() {
        if checked {
            ids.push(row.header.id.clone());
        } else {
            ids.retain(|id| id != &row.header.id);
        }
    }
}
fn choice(ui: &mut Ui, rule: &mut Value, field: &str, label: &str, values: &[&str]) {
    let mut selected = text(rule, field).to_owned();
    crate::theme::field_row(ui, label, |ui| {
        egui::ComboBox::from_id_salt(field)
            .selected_text(&selected)
            .show_ui(ui, |ui| {
                for value in values {
                    ui.selectable_value(&mut selected, (*value).to_owned(), *value);
                }
            });
    });
    rule[field] = json!(selected);
}

#[cfg(test)]
mod tests {
    use super::*;
    use artex_core::{Header, default_data, new_id};
    use std::collections::BTreeMap;
    fn row(collection: &str, prefix: &str, patch: Value) -> Record {
        let id = new_id(prefix);
        let mut data = default_data(collection).unwrap();
        for (key, value) in patch.as_object().unwrap() {
            data[key] = value.clone();
        }
        data["id"] = json!(id);
        Record {
            header: Header {
                id,
                collection: collection.into(),
                version: "1".into(),
                commit_seq: "1".into(),
                created_at: "2026-10-17T00:00:00Z".into(),
                updated_at: "2026-10-17T00:00:00Z".into(),
                legacy_key: Value::Null,
            },
            data,
        }
    }
    fn snapshot(rows: &[Record]) -> Snapshot {
        let mut s = Snapshot {
            workspace_id: new_id("workspace"),
            commit_seq: 1,
            collections: BTreeMap::new(),
        };
        for row in rows {
            s.collections
                .entry(row.header.collection.clone())
                .or_default()
                .insert(row.header.id.clone(), row.clone());
        }
        s
    }
    fn frame(
        ctx: &egui::Context,
        panes: &mut Panes,
        s: &Snapshot,
        effects: &mut Vec<Effect>,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 2400.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| panes.new_task(ui, s, effects));
            },
        )
    }
    #[test]
    fn source_template_copies_only_its_fields_and_creation_click_sends_advanced_options() {
        let category = row(
            "task_categories",
            "category",
            json!({"name":"fixture category"}),
        );
        let company = row("companies", "company", json!({"name":"fixture company"}));
        let template = row(
            "task_templates",
            "template",
            json!({"name":"fixture template","description":"template description","goal":"template goal","category_id":category.header.id,"intercept_rules":[{"enabled":true,"action":"block","kind":"exact_domain","pattern":"excluded.example","note":"template rule"}]}),
        );
        let s = snapshot(&[category.clone(), company.clone(), template.clone()]);
        let mut p = Panes::default();
        p.title = "kept title".into();
        p.creation.companies.push(company.header.id.clone());
        p.creation.timeout = "120".into();
        p.creation.heartbeat = "900".into();
        p.creation.seed_first_intent = true;
        p.creation.coverage = false;
        p.apply_task_template(template);
        assert_eq!(p.title, "kept title");
        assert_eq!(p.description, "template description");
        assert_eq!(p.goal, "template goal");
        let ctx = egui::Context::default();
        let mut effects = vec![];
        let output = frame(&ctx, &mut p, &s, &mut effects, vec![]);
        let pos = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(t) if t.galley.text() == "작업 만들기" => {
                    Some(t.visual_bounding_rect().center())
                }
                _ => None,
            })
            .expect("production create button");
        let _ = frame(
            &ctx,
            &mut p,
            &s,
            &mut effects,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        );
        let _ = frame(
            &ctx,
            &mut p,
            &s,
            &mut effects,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
        let input = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::Command { method, params, .. } if method == "ui.createTask" => {
                    Some(&params["input"])
                }
                _ => None,
            })
            .expect("creation effect");
        assert_eq!(input["companyIds"], json!([company.header.id]));
        assert_eq!(input["categoryId"], category.header.id);
        assert_eq!(input["timeoutSeconds"], 120);
        assert_eq!(input["planHeartbeatSeconds"], 900);
        assert_eq!(input["seedFirstIntent"], true);
        assert_eq!(input["coverageEnabled"], false);
        assert_eq!(input["interceptRules"][0]["pattern"], "excluded.example");
    }
    #[test]
    fn default_zero_options_are_preserved_and_invalid_or_deleted_references_keep_drafts() {
        let mut p = Panes::default();
        p.description = "fixture".into();
        p.goal = "local only".into();
        let s = snapshot(&[]);
        let input = p.task_creation_input(&s).unwrap();
        assert_eq!(input["input"]["timeoutSeconds"], 0);
        assert_eq!(input["input"]["planHeartbeatSeconds"], 0);
        assert_eq!(input["input"]["seedFirstIntent"], false);
        assert_eq!(input["input"]["coverageEnabled"], true);
        p.creation.heartbeat = "59".into();
        assert!(p.task_creation_input(&s).is_err());
        p.creation.heartbeat = "0".into();
        p.creation.timeout = "-1".into();
        assert!(p.task_creation_input(&s).is_err());
        p.creation.timeout = "0".into();
        p.creation.category = Some(new_id("category"));
        assert!(p.task_creation_input(&s).is_err());
        assert_eq!(p.description, "fixture");
        assert_eq!(p.goal, "local only");
    }
    #[test]
    fn suspension_and_readonly_keep_the_creation_draft_without_admitting_a_task() {
        let mut p = Panes::default();
        p.description = "unsaved creation".into();
        p.goal = "local fixture".into();
        p.creation.timeout = "240".into();
        p.creation.heartbeat = "1200".into();
        p.creation.seed_first_intent = true;
        p.creation.coverage = false;
        p.creation.rules.push(json!({"action":"allow","kind":"exact_domain","pattern":"local.example","enabled":true,"note":"unsaved rule"}));
        let s = snapshot(&[]);
        let before = p.task_creation_input(&s).unwrap();
        p.retire_async_operations();
        assert_eq!(p.task_creation_input(&s).unwrap(), before);
        p.set_read_only(true);
        let ctx = egui::Context::default();
        let mut effects = vec![];
        let out = frame(&ctx, &mut p, &s, &mut effects, vec![]);
        let pos = out
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(t) if t.galley.text() == "작업 만들기" => {
                    Some(t.visual_bounding_rect().center())
                }
                _ => None,
            })
            .unwrap();
        let _ = frame(
            &ctx,
            &mut p,
            &s,
            &mut effects,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        );
        let _ = frame(
            &ctx,
            &mut p,
            &s,
            &mut effects,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e,Effect::Command{method,..} if method=="ui.createTask"))
        );
        assert_eq!(p.task_creation_input(&s).unwrap(), before);
    }
    #[test]
    fn template_replace_buttons_require_an_explicit_choice_and_keep_other_creation_options() {
        let template = row(
            "task_templates",
            "template",
            json!({"name":"replacement","description":"template description","goal":"template goal","intercept_rules":[]}),
        );
        let s = snapshot(&[template.clone()]);
        let mut p = Panes::default();
        p.description = "original draft".into();
        p.goal = "original goal".into();
        p.creation.timeout = "300".into();
        let ctx = egui::Context::default();
        let mut effects = vec![];
        for apply in [false, true] {
            p.creation.pending_template = Some(template.clone());
            assert_eq!(p.description, "original draft");
            let out = frame(&ctx, &mut p, &s, &mut effects, vec![]);
            let label = if apply {
                "템플릿 적용"
            } else {
                "현재 초안 유지"
            };
            let pos = out
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(t) if t.galley.text() == label => {
                        Some(t.visual_bounding_rect().center())
                    }
                    _ => None,
                })
                .unwrap();
            let _ = frame(
                &ctx,
                &mut p,
                &s,
                &mut effects,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                ],
            );
            let _ = frame(
                &ctx,
                &mut p,
                &s,
                &mut effects,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
            );
            assert!(p.creation.pending_template.is_none());
            assert_eq!(p.creation.timeout, "300");
            assert_eq!(
                p.description,
                if apply {
                    "template description"
                } else {
                    "original draft"
                }
            );
        }
        assert!(effects.is_empty());
    }
    fn click_creation_label(
        ctx: &egui::Context,
        p: &mut Panes,
        s: &Snapshot,
        effects: &mut Vec<Effect>,
        label: &str,
    ) {
        let out = frame(ctx, p, s, effects, vec![]);
        let pos = out
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(t) if t.galley.text() == label => {
                    Some(t.visual_bounding_rect().center())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Missing control: {label}"));
        let _ = frame(
            ctx,
            p,
            s,
            effects,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        );
        let _ = frame(
            ctx,
            p,
            s,
            effects,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
    }
    #[test]
    fn deleted_company_selection_can_be_explicitly_removed_without_losing_the_draft() {
        let company = row(
            "companies",
            "company",
            json!({"name":"Deleted fixture company"}),
        );
        let kept = row(
            "companies",
            "company",
            json!({"name":"Kept fixture company"}),
        );
        let mut s = snapshot(&[company.clone(), kept.clone()]);
        let mut p = Panes::default();
        p.description = "Preserved unsaved draft".into();
        p.goal = "Local fixture only".into();
        p.creation.timeout = "120".into();
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.);
        let mut effects = vec![];
        click_creation_label(&ctx, &mut p, &s, &mut effects, "연결할 회사");
        for label in ["Deleted fixture company", "Kept fixture company"] {
            click_creation_label(&ctx, &mut p, &s, &mut effects, label);
        }
        p.set_context(crate::model::Section::Assets, None);
        s.collections
            .get_mut("companies")
            .unwrap()
            .remove(&company.header.id);
        s.commit_seq += 1;
        p.set_context(crate::model::Section::Tasks, None);
        assert!(p.task_creation_input(&s).is_err());
        // Rendering/navigation must not silently drop either selected company.
        let _ = frame(&ctx, &mut p, &s, &mut effects, vec![]);
        assert_eq!(p.creation.companies.len(), 2);
        click_creation_label(
            &ctx,
            &mut p,
            &s,
            &mut effects,
            &format!("회사 선택 해제: {}", company.header.id),
        );
        assert_eq!(p.creation.companies, vec![kept.header.id.clone()]);
        assert_eq!(p.description, "Preserved unsaved draft");
        assert_eq!(p.goal, "Local fixture only");
        assert_eq!(p.creation.timeout, "120");
        click_creation_label(&ctx, &mut p, &s, &mut effects, "작업 만들기");
        let input = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::Command { method, params, .. } if method == "ui.createTask" => {
                    Some(&params["input"])
                }
                _ => None,
            })
            .expect("Explicit removal admits creation");
        assert_eq!(input["companyIds"], json!([kept.header.id]));
        assert_eq!(input["timeoutSeconds"], 120);
    }
    #[test]
    fn missing_source_or_profile_can_be_explicitly_removed_then_creation_keeps_other_choices() {
        for unavailable in ["deleted source", "archived source", "deleted profile"] {
            let source = row("tasks", "task", json!({"name":"Selected source"}));
            let kept_source = row("tasks", "task", json!({"name":"Kept source"}));
            let profile = row(
                "llm_profiles",
                "profile",
                json!({"name":"Selected profile"}),
            );
            let kept_profile = row("llm_profiles", "profile", json!({"name":"Kept profile"}));
            let mut snapshot = snapshot(&[
                source.clone(),
                kept_source.clone(),
                profile.clone(),
                kept_profile.clone(),
            ]);
            let mut p = Panes::default();
            p.description = "Unsaved draft".into();
            p.goal = "Local fixture".into();
            p.creation.timeout = "120".into();
            let ctx = egui::Context::default();
            ctx.style_mut(|style| style.animation_time = 0.);
            let mut effects = vec![];
            click_creation_label(
                &ctx,
                &mut p,
                &snapshot,
                &mut effects,
                "정보를 공유할 직접 원본 작업",
            );
            for label in [
                "Selected source",
                "Kept source",
                "Selected profile",
                "Kept profile",
            ] {
                click_creation_label(&ctx, &mut p, &snapshot, &mut effects, label);
            }
            p.set_context(crate::model::Section::Assets, None);
            let (id, label) = if unavailable == "deleted profile" {
                snapshot
                    .collections
                    .get_mut("llm_profiles")
                    .unwrap()
                    .remove(&profile.header.id);
                (&profile.header.id, "모델")
            } else {
                if unavailable == "deleted source" {
                    snapshot
                        .collections
                        .get_mut("tasks")
                        .unwrap()
                        .remove(&source.header.id);
                } else {
                    snapshot
                        .collections
                        .get_mut("tasks")
                        .unwrap()
                        .get_mut(&source.header.id)
                        .unwrap()
                        .data["archived_at"] = json!("2026-10-17T00:00:00Z");
                }
                (&source.header.id, "원본 작업")
            };
            p.set_context(crate::model::Section::Tasks, None);
            assert!(p.task_creation_input(&snapshot).is_err());
            let _ = frame(&ctx, &mut p, &snapshot, &mut effects, vec![]);
            assert_eq!(p.selected_profiles.len(), 2);
            assert_eq!(p.selected_sources.len(), 2);
            click_creation_label(&ctx, &mut p, &snapshot, &mut effects, "작업 만들기");
            assert!(!effects.iter().any(
                |effect| matches!(effect,Effect::Command{method,..} if method=="ui.createTask")
            ));
            click_creation_label(
                &ctx,
                &mut p,
                &snapshot,
                &mut effects,
                &format!("{label} 선택 해제: {id}"),
            );
            assert_eq!(p.description, "Unsaved draft");
            assert_eq!(p.goal, "Local fixture");
            assert_eq!(p.creation.timeout, "120");
            click_creation_label(&ctx, &mut p, &snapshot, &mut effects, "작업 만들기");
            let input = effects
                .iter()
                .find_map(|effect| match effect {
                    Effect::Command { method, params, .. } if method == "ui.createTask" => {
                        Some(&params["input"])
                    }
                    _ => None,
                })
                .expect("Recovered creation effect");
            let references = &input[if label == "모델" {
                "llmProfileIds"
            } else {
                "sourceTaskIds"
            }];
            assert!(
                !references
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| value == id)
            );
            assert!(
                input["llmProfileIds"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| value == &kept_profile.header.id)
            );
            assert!(
                input["sourceTaskIds"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| value == &kept_source.header.id)
            );
        }
    }
}

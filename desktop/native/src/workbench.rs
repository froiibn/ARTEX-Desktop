//! Desktop chrome owns presentation state only. Reads and writes remain on
//! NativeApp's original single active Section and pinned command generation.
use super::*;
use crate::theme::{self, Icon};
fn shortcut(key: &str) -> String {
    crate::window_chrome::Platform::current().shortcut(key)
}

// Keep the project's legal terms available even when the machine is offline.
const AGPL_LICENSE: &str = include_str!("../../../LICENSE");
const PROJECT_SOURCE: &str = "https://github.com/froiibn/ARTEX-Desktop";
const RELEASE_SOURCE: &str = "https://github.com/froiibn/ARTEX-Desktop/releases";

fn source_url_for_commit(commit: Option<&str>) -> String {
    match commit.filter(|sha| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit())) {
        Some(sha) => format!("{PROJECT_SOURCE}/tree/{}", sha.to_ascii_lowercase()),
        None => PROJECT_SOURCE.to_owned(),
    }
}

fn license_text(ui: &mut egui::Ui) -> egui::scroll_area::ScrollAreaOutput<()> {
    egui::ScrollArea::vertical()
        .id_salt("local_agpl_license_text")
        .max_height(220.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(egui::Label::new(AGPL_LICENSE).wrap().selectable(true));
        })
}

fn about_contents(ui: &mut egui::Ui) {
    ui.heading("ARTEX Desktop");
    ui.label("Rust 통합형 데스크톱 · 로컬 작업 공간");
    ui.label(format!("버전 {}", env!("CARGO_PKG_VERSION")));
    ui.separator();
    ui.strong("단축키");
    ui.label(format!("{}  작업 공간 열기", shortcut("O")));
    ui.label(format!(
        "{}  체크포인트 기록 (편집 초안 저장과 별개)",
        shortcut("S")
    ));
    ui.label(format!("{}  현재 항목 목록 검색", shortcut("F")));
    ui.label(format!("{}  실행 기록 패널 표시", shortcut("J")));
    ui.label(format!("{}  저장·종료 확인", shortcut("Q")));
    ui.separator();
    ui.strong("저작권과 라이선스");
    ui.label("원작 ARTEX 저작권: Autumn-27 및 원본 기여자. 원본 소스 백업: mhtsec/ARTEX.");
    ui.label("ARTEX Desktop은 원작을 수정·재구축한 버전입니다. 원작자와 기여자의 권리는 해당 권리자에게 있습니다.");
    ui.small("수정 기준: 2026-10-14");
    ui.label("이 프로그램은 AGPL-3.0-only에 따라 사용할 수 있으며, 해당 조건을 준수하여 수정하고 재배포할 수 있습니다.");
    ui.label("이 프로그램은 어떠한 보증 없이 현 상태로 제공됩니다. 상품성 또는 특정 목적에 대한 적합성도 보증하지 않습니다. 자세한 조건과 책임 제한은 아래 원문을 확인하세요.");
    ui.hyperlink_to(
        "프로젝트 소스와 수정 이력",
        source_url_for_commit(option_env!("ARTEX_SOURCE_COMMIT")),
    );
    ui.hyperlink_to("릴리스별 대응 소스 다운로드", RELEASE_SOURCE);
    ui.small("실행 파일을 받은 릴리스의 같은 태그·소스 ZIP과 빌드 지침을 사용하세요. 저장소가 비공개인 동안에는 접근 권한이 필요합니다.");
    egui::CollapsingHeader::new("AGPL-3.0-only 원문 보기 · 앱에 포함됨")
        .id_salt("local_agpl_license")
        .show(ui, license_text);
    ui.separator();
    ui.small("Linux 데스크톱에서 영감을 받은 자체 Rust 테마");
}

fn about_window(ctx: &egui::Context, open: &mut bool) {
    let viewport = ctx.input(|input| input.content_rect());
    let width = (viewport.width() - 64.0).clamp(200.0, 480.0);
    egui::Window::new("ARTEX 정보 · 라이선스")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(width)
        .max_width(width)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("artex_about_contents")
                .max_height((viewport.height() - 120.0).max(120.0))
                .auto_shrink([false, true])
                .show(ui, about_contents);
        });
}

#[cfg(test)]
mod legal_tests {
    use super::*;

    #[test]
    fn about_displays_original_author_rights_and_source_access_without_a_workspace() {
        let ctx = egui::Context::default();
        let mut open = true;
        let mut rendered = Vec::new();
        for _ in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ctx| about_window(ctx, &mut open),
            );
            rendered = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        for required in [
            "Autumn-27",
            "수정·재구축한 버전",
            "수정 기준: 2026-10-14",
            "AGPL-3.0-only",
            "수정하고 재배포",
            "어떠한 보증 없이",
            "프로젝트 소스와 수정 이력",
            "릴리스별 대응 소스 다운로드",
            "원문 보기 · 앱에 포함됨",
            "작업 공간 열기",
            "체크포인트 기록",
        ] {
            assert!(
                rendered.iter().any(|text| text.contains(required)),
                "About window omitted {required}"
            );
        }
        assert!(open);
    }

    #[test]
    fn complete_local_license_is_readable_in_a_bounded_scroll_area() {
        assert!(AGPL_LICENSE.contains("GNU AFFERO GENERAL PUBLIC LICENSE"));
        assert!(AGPL_LICENSE.contains("13. Remote Network Interaction"));
        assert!(AGPL_LICENSE.contains("END OF TERMS AND CONDITIONS"));
        let ctx = egui::Context::default();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 320.0));
        let mut inner_rect = egui::Rect::NOTHING;
        let mut offset = 0.0;
        for pass in 0..6 {
            let events = if pass == 1 {
                vec![
                    egui::Event::PointerMoved(inner_rect.center()),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -120.0),
                        modifiers: egui::Modifiers::default(),
                    },
                ]
            } else {
                vec![]
            };
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    time: Some(f64::from(pass) / 60.0),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    let scroll = egui::CentralPanel::default().show(ctx, license_text).inner;
                    inner_rect = scroll.inner_rect;
                    offset = scroll.state.offset.y;
                    assert!(scroll.content_size.y > inner_rect.height());
                },
            );
            let license = output
                .shapes
                .iter()
                .find(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == AGPL_LICENSE))
                .expect("The app must render the complete bundled license without a web request");
            // egui expands the painter clip by the style's margin to protect
            // strokes and glyphs; the allocated scroll viewport itself is 220px.
            let margin = ctx.style().visuals.clip_rect_margin;
            assert!(inner_rect.height() <= 220.0);
            assert!(viewport.contains_rect(inner_rect));
            assert!(license.clip_rect.top() >= inner_rect.top() - margin);
            assert!(license.clip_rect.bottom() <= inner_rect.bottom() + margin);
            assert!(license.clip_rect.width() <= viewport.width());
        }
        assert!(
            offset > 0.0,
            "The complete license must actually scroll on wheel input"
        );
    }

    #[test]
    fn source_link_pins_valid_build_commit_and_rejects_non_commit_values() {
        let sha = "0123456789ABCDEF0123456789ABCDEF01234567";
        assert_eq!(
            source_url_for_commit(Some(sha)),
            format!("{PROJECT_SOURCE}/tree/{}", sha.to_ascii_lowercase())
        );
        for value in [
            None,
            Some(""),
            Some("main"),
            Some("../other"),
            Some("01234567"),
            Some("gggggggggggggggggggggggggggggggggggggggg"),
        ] {
            assert_eq!(source_url_for_commit(value), PROJECT_SOURCE);
        }
    }

    #[test]
    fn small_about_window_keeps_title_and_shortcuts_inside_the_viewport() {
        let ctx = egui::Context::default();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 480.0));
        let mut open = true;
        let mut title_visible = false;
        let mut shortcut_visible = false;
        for _ in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ctx| about_window(ctx, &mut open),
            );
            title_visible = output.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "ARTEX 정보 · 라이선스" => {
                    viewport.contains_rect(text.visual_bounding_rect())
                }
                _ => false,
            });
            shortcut_visible = output.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text().contains("작업 공간 열기") => {
                    viewport.contains_rect(text.visual_bounding_rect())
                        && shape.clip_rect.contains_rect(text.visual_bounding_rect())
                }
                _ => false,
            });
        }
        assert!(
            title_visible,
            "About title must remain visible in a short viewport"
        );
        assert!(
            shortcut_visible,
            "Existing keyboard shortcuts must remain visible"
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawerTab {
    Activity,
    Management,
}
impl DrawerTab {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Activity => "실행·변경 기록",
            Self::Management => "추가 관리",
        }
    }
}
#[derive(Default)]
pub(crate) struct CompactToolbarPresentation<'a> {
    pub workspace_name: Option<&'a str>,
    pub workspace_path: Option<&'a str>,
    pub read_only: bool,
    pub bypass: Option<(bool, bool, bool)>,
    pub list_open: Option<bool>,
    pub details_open: Option<bool>,
    pub workspace_choices_enabled: bool,
    pub settings_records: Option<bool>,
    pub management_open: Option<bool>,
}
#[derive(Clone, Copy)]
pub(crate) enum ChromeAction {
    Open,
    New,
    ToggleBypass,
    List,
    GeneralSettings,
    AdvancedSettings,
    Management,
    Details,
}
fn workspace_buttons(ui: &mut egui::Ui, enabled: bool) -> Vec<ChromeAction> {
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal(|ui| {
            let mut actions = vec![];
            if theme::secondary_button(ui, "열기")
                .on_hover_text(format!("기존 작업 공간으로 전환 · {}", shortcut("O")))
                .clicked()
            {
                actions.push(ChromeAction::Open);
            }
            if theme::secondary_button(ui, "새 공간")
                .on_hover_text("현재 공간을 저장·정리한 뒤 새 작업 공간을 만듭니다.")
                .clicked()
            {
                actions.push(ChromeAction::New);
            }
            actions
        })
        .inner
    })
    .inner
}

pub(crate) fn compact_toolbar(
    ui: &mut egui::Ui,
    state: &CompactToolbarPresentation<'_>,
) -> Vec<ChromeAction> {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.spacing_mut().button_padding = egui::vec2(10.0, 5.0);
        ui.spacing_mut().interact_size.y = 28.0;
        ui.horizontal(|ui| {
            let mut actions = Vec::new();
            actions.extend(workspace_buttons(ui, state.workspace_choices_enabled));
            if let Some(name) = state.workspace_name {
                ui.add_sized([112.0, 28.0], egui::Label::new(name).truncate())
                    .on_hover_text(state.workspace_path.unwrap_or(name));
            } else {
                ui.weak("미선택");
            }
            if state.read_only {
                ui.small("읽기 전용");
            }
            if let Some((enabled, pending, writable)) = state.bypass {
                ui.menu_button(
                    RichText::new(if enabled {
                        "Bypass 켜짐"
                    } else {
                        "Bypass 꺼짐"
                    })
                    .color(if enabled {
                        ui.visuals().warn_fg_color
                    } else {
                        theme::muted(ui)
                    }),
                    |ui| {
                        ui.label("현재 작업 공간의 내부 실행 승인 생략");
                        if ui
                            .add_enabled(
                                writable && !pending,
                                egui::Button::new(if enabled {
                                    "Bypass 끄기"
                                } else {
                                    "Bypass 켜기"
                                }),
                            )
                            .clicked()
                        {
                            actions.push(ChromeAction::ToggleBypass);
                            ui.close();
                        }
                    },
                );
                if pending {
                    ui.spinner();
                }
            }
            if let Some(advanced) = state.settings_records {
                if ui.selectable_label(!advanced, "일반 설정").clicked() {
                    actions.push(ChromeAction::GeneralSettings);
                }
                if ui
                    .selectable_label(advanced, "고급")
                    .on_hover_text("고급 내부 레코드 목록")
                    .clicked()
                {
                    actions.push(ChromeAction::AdvancedSettings);
                }
            }
            if let Some(open) = state.list_open {
                if ui
                    .selectable_label(open, "목록")
                    .on_hover_text(format!("항목 목록 열기/접기 · {}", shortcut("F")))
                    .clicked()
                {
                    actions.push(ChromeAction::List);
                }
            }
            if let Some(open) = state.management_open {
                if ui
                    .selectable_label(open, "관리")
                    .on_hover_text("대화 추가 관리 열기/접기")
                    .clicked()
                {
                    actions.push(ChromeAction::Management);
                }
            }
            if let Some(open) = state.details_open {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new("상세").selected(open).frame(true))
                        .clicked()
                    {
                        actions.push(ChromeAction::Details);
                    }
                });
            }
            actions
        })
        .inner
    })
    .inner
}
pub(super) struct DrawerState {
    pub(super) open: bool,
    pub(super) tab: DrawerTab,
    pub(super) last_rect: Option<egui::Rect>,
    pub(super) body_rect: Option<egui::Rect>,
}
impl Default for DrawerState {
    fn default() -> Self {
        Self {
            open: false,
            tab: DrawerTab::Activity,
            last_rect: None,
            body_rect: None,
        }
    }
}
impl DrawerState {
    pub(super) fn show(&mut self, tab: DrawerTab) {
        self.open = true;
        self.tab = tab;
    }
    pub(super) fn toggle(&mut self, tab: DrawerTab) {
        if self.open && self.tab == tab {
            self.open = false;
        } else {
            self.show(tab);
        }
    }
}
fn drawer_height_limit(available: f32, viewport_height: f32, minimum_content: f32) -> f32 {
    (available - minimum_content)
        .max(110.0)
        .min(if viewport_height <= 680.0 {
            120.0
        } else {
            360.0
        })
}
pub(crate) fn drawer_panel<R>(
    ctx: &egui::Context,
    minimum_content: f32,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let viewport_height = ctx.input(|i| i.content_rect().height());
    let limit = drawer_height_limit(
        ctx.available_rect().height(),
        viewport_height,
        minimum_content,
    );
    egui::TopBottomPanel::bottom("workbench_drawer")
        .resizable(true)
        .default_height(
            if viewport_height <= 680.0 {
                120.0_f32
            } else {
                200.0_f32
            }
            .min(limit),
        )
        .height_range(110.0..=limit)
        .frame(theme::toolbar_frame(&ctx.style()).inner_margin(egui::Margin::symmetric(14, 4)))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.spacing_mut().button_padding.y = 4.0;
            ui.spacing_mut().interact_size.y = 26.0;
            contents(ui)
        })
}

pub(crate) fn drawer_scroll<R>(
    ui: &mut egui::Ui,
    tab: DrawerTab,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    egui::ScrollArea::vertical()
        .id_salt(("drawer-content", tab == DrawerTab::Activity))
        .auto_shrink([false, false])
        // egui's default 64px scroll floor would grow a compact panel after
        // its header and frame margins have consumed part of the height.
        .min_scrolled_height(0.0)
        .max_height(ui.available_height().max(0.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            contents(ui)
        })
}

fn workspace_bypass(snapshot: &artex_core::Snapshot) -> bool {
    snapshot
        .records("settings")
        .into_iter()
        .find(|r| r.data["key"] == "approval_bypass")
        .is_some_and(|r| r.data["value"] == true || r.data["value"].as_str() == Some("true"))
}
fn bypass_pending(purpose: &Purpose, scope: &WorkspaceScope) -> bool {
    match purpose {
        Purpose::BypassToggle { scope: owner, .. } => owner == scope,
        Purpose::Prepare { then, .. } => bypass_pending(then, scope),
        _ => false,
    }
}

fn background_observation(purpose: &Purpose) -> bool {
    matches!(
        purpose,
        Purpose::Projection { .. }
            | Purpose::NativeOperation {
                mutation: false,
                ..
            }
            | Purpose::FileRead { .. }
            | Purpose::SourceBody { .. }
    )
}

// The toolbar and inspector resolve exactly the same visible, supported record.
pub(super) fn inspector_item<'a>(
    snapshot: &'a artex_core::Snapshot,
    section: Section,
    id: Option<&str>,
) -> Option<&'a artex_core::Record> {
    if section.configuration() {
        return None;
    }
    selected_record(snapshot, section, id).or_else(|| {
        (section == Section::Chat)
            .then(|| selected_record(snapshot, Section::Tasks, id))
            .flatten()
    })
}

// Presentation preferences are independent per section. Conversation reading
// starts wide; opening details never changes selection or the editor's owner.
fn inspector_preference(preferences: &BTreeMap<Section, bool>, section: Section) -> bool {
    preferences
        .get(&section)
        .copied()
        .unwrap_or(section != Section::Chat)
}

#[derive(Default)]
pub(super) struct ListPreferences {
    compact: bool,
    // Narrow-window temporary choices must not collapse a wide-window list.
    sections: BTreeMap<Section, (Option<bool>, Option<bool>)>,
    settings_records: bool,
}
impl ListPreferences {
    fn update_width(&mut self, width: f32) {
        if width.is_finite() {
            self.compact = if self.compact {
                width < 1180.0
            } else {
                width < 1100.0
            };
        }
    }
    fn open(&self, section: Section) -> bool {
        if section == Section::Settings && !self.settings_records {
            return false;
        }
        let remembered = self
            .sections
            .get(&section)
            .and_then(|choice| if self.compact { choice.0 } else { choice.1 });
        remembered.unwrap_or(!self.compact)
    }
    fn set_open(&mut self, section: Section, open: bool) {
        let choice = self.sections.entry(section).or_default();
        if self.compact {
            choice.0 = Some(open);
        } else {
            choice.1 = Some(open);
        }
    }
    fn selected(&mut self, section: Section) {
        if self.compact {
            self.set_open(section, false);
        }
    }
    fn general_settings(&mut self) {
        self.settings_records = false;
    }
    fn advanced_settings(&mut self) {
        self.settings_records = true;
        self.set_open(Section::Settings, true);
    }
}

pub(super) fn presentation_name(
    snapshot: &artex_core::Snapshot,
    section: Section,
    record: &artex_core::Record,
) -> String {
    let label = execution_label(snapshot, section, record);
    if matches!(section, Section::Approvals | Section::Sessions) {
        format!(
            "{label} · {}",
            crate::panes::presentation_time(&record.header.created_at)
        )
    } else {
        label
    }
}
fn list_presentation_name(
    snapshot: &artex_core::Snapshot,
    section: Section,
    record: &artex_core::Record,
) -> String {
    let label = execution_label(snapshot, section, record);
    if matches!(section, Section::Approvals | Section::Sessions) {
        format!(
            "{label}\n{}",
            crate::panes::presentation_short_time(&record.header.created_at)
        )
    } else {
        label
    }
}
fn object_list_button(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let width = ui.available_width();
    if let Some((title, clock)) = label.split_once('\n') {
        // Truncate the title independently: egui's whole-label Truncate mode
        // has max_rows=1 and would discard the distinguishing time line.
        let title_galley = egui::WidgetText::from(title).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            (width - 2.0 * ui.spacing().button_padding.x).max(1.0),
            egui::TextStyle::Button,
        );
        let title = title_galley
            .rows
            .first()
            .map(|row| {
                row.row
                    .glyphs
                    .iter()
                    .map(|glyph| glyph.chr)
                    .collect::<String>()
            })
            .unwrap_or_default();
        ui.add(
            egui::Button::selectable(selected, format!("{title}\n{clock}"))
                .wrap_mode(egui::TextWrapMode::Extend)
                .stroke(egui::Stroke::NONE)
                .corner_radius(8)
                .min_size(egui::vec2(width, 52.0)),
        )
    } else {
        ui.add(
            egui::Button::selectable(selected, label)
                .truncate()
                .stroke(egui::Stroke::NONE)
                .corner_radius(8)
                .min_size(egui::vec2(width, 36.0)),
        )
    }
}

fn object_list_row(
    ui: &mut egui::Ui,
    panes: &mut Panes,
    snapshot: &artex_core::Snapshot,
    section: Section,
    record: &artex_core::Record,
    selected: bool,
    label: &str,
) -> egui::Response {
    let has_actions = matches!(section, Section::Chat | Section::Tasks | Section::Archives);
    ui.push_id(&record.header.id, |ui| {
        ui.horizontal(|ui| {
            panes.object_batch_checkbox(ui, section, record);
            // Reserve the action button AND the spacing before it. Otherwise
            // the row grows the SidePanel beyond its painted/clip rectangle,
            // leaving an unpainted strip between the list and the main pane.
            let action_width = if has_actions {
                let text = egui::WidgetText::from("⋯").into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    egui::TextStyle::Button,
                );
                (text.size().x + 2.0 * ui.spacing().button_padding.x)
                    .max(ui.spacing().interact_size.x)
                    + ui.spacing().item_spacing.x
            } else {
                0.0
            };
            let width = (ui.available_width() - action_width).max(1.0);
            let response = ui
                .allocate_ui_with_layout(
                    egui::vec2(width, if label.contains('\n') { 52.0 } else { 36.0 }),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| object_list_button(ui, selected, label),
                )
                .inner
                .on_hover_text(format!(
                    "{}\n{}",
                    presentation_name(snapshot, section, record),
                    record.header.id
                ));
            if has_actions {
                response.context_menu(|ui| panes.object_actions(ui, snapshot, section, record));
                ui.menu_button("⋯", |ui| {
                    panes.object_actions(ui, snapshot, section, record)
                });
            }
            response
        })
        .inner
    })
    .inner
}
fn execution_label(
    snapshot: &artex_core::Snapshot,
    section: Section,
    record: &artex_core::Record,
) -> String {
    let state = record.data["state"]
        .as_str()
        .or_else(|| record.data["status"].as_str())
        .unwrap_or("");
    let name = match section {
        Section::Approvals => record.data["tool_name"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("도구 실행 승인")
            .to_owned(),
        Section::Sessions => record.data["agent_id"]
            .as_str()
            .and_then(|id| snapshot.get("agents", id))
            .map(display_name)
            .filter(|label| record.data["agent_id"].as_str() != Some(label.as_str()))
            .or_else(|| {
                record.data["agent_key"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "에이전트 실행".into()),
        _ => return display_name(record),
    };
    format!("{name} · {}", display_state(state))
}

#[cfg(test)]
mod list_boundary_tests {
    use super::*;

    fn fixture(section: Section) -> (artex_core::Snapshot, artex_core::Record) {
        let collection = section.collection().unwrap();
        let record = serde_json::from_value(json!({
            "header": {"id": "list-boundary-fixture", "collection": collection,
                "version": "1", "commit_seq": "1", "created_at": "2026-10-15T00:00:00Z",
                "updated_at": "2026-10-15T00:00:00Z", "legacy_key": null},
            "data": {"name": "A very long conversation or task name that must stay inside the list"}
        }))
        .unwrap();
        (
            artex_core::Snapshot {
                workspace_id: "boundary-test".into(),
                commit_seq: 1,
                collections: Default::default(),
            },
            record,
        )
    }

    #[test]
    fn list_rows_do_not_grow_panel_or_leave_an_unpainted_boundary() {
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            for section in [Section::Chat, Section::Tasks, Section::Archives] {
                for width in [180.0, 218.0, 350.0] {
                    let ctx = egui::Context::default();
                    theme::apply(&ctx);
                    ctx.set_theme(theme);
                    let (snapshot, record) = fixture(section);
                    let mut panes = Panes::default();
                    // Read-only labels and action menus must use the same geometry.
                    panes.set_read_only(true);
                    for pass in 0..8 {
                        let mut panel_rect = egui::Rect::NOTHING;
                        let output = ctx.run(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(1000.0, 600.0),
                                )),
                                time: Some(f64::from(pass) / 60.0),
                                ..Default::default()
                            },
                            |ctx| {
                                panel_rect = egui::SidePanel::left("boundary-list")
                                    .default_width(width)
                                    .width_range(180.0..=350.0)
                                    .frame(theme::toolbar_frame(&ctx.style()))
                                    .show(ctx, |ui| {
                                        ui.strong("대화 목록");
                                        panes.object_batch_toolbar(ui, &snapshot, section);
                                        egui::ScrollArea::vertical().show(ui, |ui| {
                                            object_list_row(
                                                ui,
                                                &mut panes,
                                                &snapshot,
                                                section,
                                                &record,
                                                true,
                                                record.data["name"].as_str().unwrap(),
                                            );
                                        });
                                    })
                                    .response
                                    .rect;
                                let central = egui::CentralPanel::default().show(ctx, |ui| {
                                    ui.label("messages");
                                });
                                assert!(
                                    (central.response.rect.left() - panel_rect.right()).abs() < 0.1
                                );
                            },
                        );
                        assert!(
                            (panel_rect.width() - width).abs() < 0.1,
                            "{theme:?} {section:?} pass {pass}: panel drifted from {width} to {}",
                            panel_rect.width()
                        );
                        let painted = output.shapes.iter().find(|shape| matches!(&shape.shape,
                            egui::Shape::Rect(rect) if rect.fill == ctx.style().visuals.panel_fill && rect.rect.height() > 500.0)).unwrap();
                        let egui::Shape::Rect(rect) = &painted.shape else {
                            unreachable!()
                        };
                        assert!(painted.clip_rect.right() >= panel_rect.right() - 0.1);
                        assert!(
                            rect.rect.right() >= panel_rect.right() - 0.1,
                            "The panel must paint all the way to the main pane"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn list_boundary_still_resizes_on_drag() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let (snapshot, record) = fixture(Section::Chat);
        let mut panes = Panes::default();
        let mut widths = Vec::new();
        for pass in 0..6 {
            let pointer = egui::pos2(if pass < 3 { 218.0 } else { 278.0 }, 300.0);
            let mut events = vec![egui::Event::PointerMoved(pointer)];
            if pass == 2 || pass == 5 {
                events.push(egui::Event::PointerButton {
                    pos: pointer,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 2,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 600.0),
                    )),
                    time: Some(f64::from(pass) / 60.0),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    let panel = egui::SidePanel::left("resizable-list")
                        .resizable(true)
                        .default_width(218.0)
                        .width_range(180.0..=350.0)
                        .frame(theme::toolbar_frame(&ctx.style()))
                        .show(ctx, |ui| {
                            object_list_row(
                                ui,
                                &mut panes,
                                &snapshot,
                                Section::Chat,
                                &record,
                                false,
                                "대화",
                            );
                        });
                    widths.push(panel.response.rect.width());
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.label("messages");
                    });
                },
            );
        }
        assert!((widths[0] - 218.0).abs() < 0.1);
        assert!((widths[5] - 278.0).abs() < 0.1, "Resize failed: {widths:?}");
    }
}

#[cfg(test)]
mod bypass_tests {
    use super::*;
    #[test]
    fn drawer_tabs_are_exclusive_default_closed_and_keyboard_activity_toggle_is_predictable() {
        let mut drawer = DrawerState::default();
        assert!(!drawer.open);
        drawer.toggle(DrawerTab::Management);
        assert!(drawer.open && drawer.tab == DrawerTab::Management);
        drawer.toggle(DrawerTab::Activity);
        assert!(drawer.open && drawer.tab == DrawerTab::Activity);
        drawer.toggle(DrawerTab::Activity);
        assert!(!drawer.open);
        drawer.show(DrawerTab::Management);
        assert!(drawer.open && drawer.tab == DrawerTab::Management);
    }

    #[test]
    fn drawer_allocates_full_width_before_sidebar_and_reserves_minimum_content_height() {
        for size in [egui::vec2(900.0, 620.0), egui::vec2(1320.0, 860.0)] {
            for tab in [DrawerTab::Activity, DrawerTab::Management] {
                let ctx = egui::Context::default();
                theme::apply(&ctx);
                let marker_color = egui::Color32::from_rgb(75, 131, 207);
                let mut drawer_rect = egui::Rect::NOTHING;
                let mut marker = egui::Rect::NOTHING;
                let mut central_height = 0.0;
                let content_minimum = Panes::default().chat_minimum_height();
                let mut toolbar_rect = egui::Rect::NOTHING;
                let mut status_rect = egui::Rect::NOTHING;
                let mut body_rect = egui::Rect::NOTHING;
                for _ in 0..2 {
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::TopBottomPanel::top("test_menu")
                                .frame(theme::toolbar_frame(&ctx.style()))
                                .show(ctx, |ui| {
                                    egui::MenuBar::new().ui(ui, |ui| {
                                        for label in ["파일", "보기", "설정", "도움말"] {
                                            let _ = ui.menu_button(label, |_| {});
                                        }
                                    });
                                });
                            toolbar_rect = egui::TopBottomPanel::top("test_toolbar")
                                .frame(theme::toolbar_frame(&ctx.style()))
                                .show(ctx, |ui| {
                                    let actions = compact_toolbar(
                                        ui,
                                        &CompactToolbarPresentation {
                                            workspace_name: Some(
                                                "native-707bcb378b1d435fb4e0b662e6ca6679",
                                            ),
                                            read_only: true,
                                            bypass: Some((true, false, false)),
                                            list_open: Some(false),
                                            details_open: Some(false),
                                            management_open: Some(tab == DrawerTab::Management),
                                            ..Default::default()
                                        },
                                    );
                                    assert!(actions.is_empty());
                                })
                                .response
                                .rect;
                            status_rect = egui::TopBottomPanel::bottom("test_status")
                                .frame(theme::status_frame(&ctx.style()))
                                .show(ctx, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.small("로컬 작업 공간");
                                        ui.separator();
                                        ui.small("저장 세대 79");
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                let _ =
                                                    ui.selectable_label(false, "실행·변경 기록");
                                            },
                                        );
                                    });
                                })
                                .response
                                .rect;
                            let panel = drawer_panel(ctx, content_minimum + 40.0, |ui| {
                                ui.horizontal(|ui| {
                                    let _ = ui.selectable_label(
                                        tab == DrawerTab::Activity,
                                        "실행·변경 기록",
                                    );
                                    let _ = ui.selectable_label(
                                        tab == DrawerTab::Management,
                                        "추가 관리",
                                    );
                                });
                                let _ = ui.add(egui::Separator::default().spacing(4.0));
                                let before = ui.available_rect_before_wrap();
                                let scroll = drawer_scroll(ui, tab, |ui| {
                                    for _ in 0..60 {
                                        ui.add(egui::Label::new("ok").wrap().selectable(true));
                                    }
                                    // A passive mark records the content geometry
                                    // after short labels; it must not allocate or
                                    // force the width under test itself.
                                    marker = egui::Rect::from_min_size(
                                        ui.min_rect().min,
                                        egui::vec2(ui.min_rect().width(), 3.0),
                                    );
                                    ui.painter().rect_filled(marker, 0, marker_color);
                                });
                                body_rect = egui::Rect::from_min_max(
                                    egui::pos2(scroll.inner_rect.left(), before.top()),
                                    egui::pos2(
                                        scroll.inner_rect.right(),
                                        ui.cursor().min.y - ui.spacing().item_spacing.y,
                                    ),
                                )
                                .intersect(ui.clip_rect());
                            });
                            drawer_rect = panel.response.rect;
                            egui::SidePanel::left("test_navigation")
                                .exact_width(190.0)
                                .show(ctx, |ui| {
                                    ui.label("대화");
                                });
                            egui::CentralPanel::default()
                                .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(20))
                                .show(ctx, |ui| {
                                    central_height = ui.available_height();
                                });
                        },
                    );
                    assert!(
                        (drawer_rect.width() - size.x).abs() < 1.0,
                        "drawer lost full window width"
                    );
                    assert!(
                        central_height >= content_minimum - 1.0,
                        "drawer left only {central_height}px for transcript/composer"
                    );
                    if size.y <= 680.0 {
                        assert!(drawer_rect.height() <= 120.0 + 1.0);
                    }
                    assert!(
                        drawer_rect.bottom() <= status_rect.top() + 1.0,
                        "drawer frame grew into the status bar: {drawer_rect:?}"
                    );
                    assert!(
                        body_rect.height() >= 60.0 - 1.0,
                        "visible drawer body collapsed to {}px",
                        body_rect.height()
                    );
                    assert!(
                        toolbar_rect.height() <= 48.0,
                        "compact toolbar wrapped into multiple rows: {toolbar_rect:?}"
                    );
                    let labels = ["열기", "새 공간", "Bypass 켜짐", "목록", "관리", "상세"];
                    let positions: Vec<_> = output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::Shape::Text(text)
                                if labels.contains(&text.galley.text())
                                    && toolbar_rect.contains(text.pos) =>
                            {
                                Some(text.pos.y)
                            }
                            _ => None,
                        })
                        .collect();
                    assert_eq!(
                        positions.len(),
                        labels.len(),
                        "compact toolbar controls were lost"
                    );
                    let top = positions.iter().copied().fold(f32::INFINITY, f32::min);
                    let bottom = positions.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                    assert!(
                        bottom - top < 8.0,
                        "actual toolbar glyphs did not stay on one row"
                    );
                    // Inspect the actually emitted geometry, not the scroll
                    // area's pre-finalization inner_rect (which may shrink).
                    let painted = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Rect(rect) if rect.fill == marker_color => Some(rect.rect),
                            _ => None,
                        })
                        .expect("scroll content was not painted");
                    assert!(
                        painted.width() > size.x - 65.0,
                        "{tab:?} content collapsed to {}px",
                        painted.width()
                    );
                    assert!(painted.right() > size.x - 45.0);
                    assert!(drawer_rect.contains_rect(marker));
                    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Rect(rect) if rect.rect.width() <= 20.0 && rect.rect.height() > 20.0 && rect.rect.right() > size.x - 35.0 && drawer_rect.intersects(rect.rect))), "scrollbar was not painted at the full-width drawer edge");
                }
            }
        }
    }
    #[test]
    fn narrow_list_choices_are_section_local_and_do_not_replace_wide_defaults() {
        let mut preferences = ListPreferences::default();
        preferences.update_width(900.0);
        assert!(!preferences.open(Section::Chat));
        preferences.set_open(Section::Chat, true);
        assert!(preferences.open(Section::Chat));
        assert!(!preferences.open(Section::Tasks));
        // Selecting a row closes only the temporary narrow-window list.
        preferences.selected(Section::Chat);
        assert!(!preferences.open(Section::Chat));
        preferences.update_width(1140.0);
        assert!(preferences.compact, "resize jitter must not switch layouts");
        preferences.update_width(1180.0);
        assert!(preferences.open(Section::Chat));
        preferences.selected(Section::Chat);
        assert!(
            preferences.open(Section::Chat),
            "wide selection keeps the list available"
        );
        preferences.set_open(Section::Chat, false);
        assert!(preferences.open(Section::Tasks));
        preferences.update_width(1120.0);
        assert!(!preferences.compact);
        preferences.update_width(900.0);
        preferences.set_open(Section::Chat, true);
        preferences.update_width(1440.0);
        assert!(
            !preferences.open(Section::Chat),
            "wide user choice must survive a temporary narrow choice"
        );
    }

    #[test]
    fn general_settings_default_retains_explicit_advanced_record_access() {
        let mut preferences = ListPreferences::default();
        preferences.update_width(1440.0);
        assert!(!preferences.open(Section::Settings));
        preferences.advanced_settings();
        assert!(preferences.open(Section::Settings));
        preferences.general_settings();
        assert!(!preferences.open(Section::Settings));
        preferences.update_width(900.0);
        preferences.advanced_settings();
        assert!(preferences.open(Section::Settings));
        preferences.selected(Section::Settings);
        assert!(!preferences.open(Section::Settings));
        assert!(
            preferences.settings_records,
            "selection still owns the advanced view"
        );
        preferences.set_open(Section::Settings, true);
        assert!(preferences.open(Section::Settings));
    }

    #[test]
    fn general_and_advanced_settings_keep_separate_existing_editor_owners() {
        let mut snapshot = artex_core::Snapshot {
            workspace_id: "settings-view-fixture".into(),
            commit_seq: 1,
            collections: Default::default(),
        };
        let id = artex_core::new_id("setting");
        let record: artex_core::Record = serde_json::from_value(json!({"header":{"id":id,"collection":"settings","version":"1","commit_seq":"1","created_at":"2026-10-13T12:34:56Z","updated_at":"2026-10-13T12:34:56Z","legacy_key":null},"data":{"key":"approval_bypass","value":false}})).unwrap();
        snapshot
            .collections
            .entry("settings".into())
            .or_default()
            .insert(id.clone(), record);
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut forms = Forms::default();
        forms.set_allowed_operations(Forms::all_known_operations());
        let mut preferences = ListPreferences::default();
        preferences.update_width(900.0);
        let mut show = |selected: Option<&str>| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = forms.show(ui, "settings", &snapshot, selected);
                });
            });
            forms.active_token().unwrap().to_owned()
        };
        let general_owner = show(None);
        preferences.advanced_settings();
        let advanced_owner = show(Some(&id));
        assert_ne!(general_owner, advanced_owner);
        preferences.selected(Section::Settings);
        assert_eq!(
            show(Some(&id)),
            advanced_owner,
            "folding the list must not switch the selected editor"
        );
        preferences.general_settings();
        assert_eq!(
            show(None),
            general_owner,
            "general view must restore its retained editor"
        );
        preferences.advanced_settings();
        assert_eq!(
            show(Some(&id)),
            advanced_owner,
            "advanced editor must remain reachable and retained"
        );
        assert_eq!(snapshot.get("settings", &id).unwrap().header.version, "1");
        assert_eq!(snapshot.get("settings", &id).unwrap().data["value"], false);
    }

    #[test]
    fn narrow_window_reclaims_list_width_and_can_reopen_then_close_after_selection() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut preferences = ListPreferences::default();
        let mut central_width = 0.0;
        let mut draw = |preferences: &mut ListPreferences, width: f32| {
            preferences.update_width(width);
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 700.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::SidePanel::left("application_navigation")
                        .default_width(190.0)
                        .width_range(166.0..=235.0)
                        .frame(theme::toolbar_frame(&ctx.style()))
                        .show(ctx, |ui| {
                            ui.label("작업 공간");
                        });
                    if preferences.open(Section::Chat) {
                        egui::SidePanel::left("section_object_list")
                            .default_width(218.0)
                            .width_range(180.0..=350.0)
                            .frame(theme::toolbar_frame(&ctx.style()))
                            .show(ctx, |ui| {
                                ui.label("대화 목록");
                            });
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        central_width = ui.available_width();
                    });
                },
            );
            central_width
        };
        let reading_width = draw(&mut preferences, 900.0);
        assert!(
            reading_width >= 650.0,
            "compact layout left only {reading_width}px for content"
        );
        preferences.set_open(Section::Chat, true);
        let choosing_width = draw(&mut preferences, 900.0);
        assert!(reading_width - choosing_width >= 180.0);
        preferences.selected(Section::Chat);
        let restored = draw(&mut preferences, 900.0);
        assert!((restored - reading_width).abs() < 1.0);
        let wide = draw(&mut preferences, 1440.0);
        assert!(preferences.open(Section::Chat));
        assert!(
            wide >= 900.0,
            "wide layout lost the existing usable list/content geometry"
        );
    }

    #[test]
    fn approval_and_session_labels_use_human_context_without_leaking_ids_into_titles() {
        let mut snapshot = artex_core::Snapshot {
            workspace_id: "display-fixture".into(),
            commit_seq: 1,
            collections: Default::default(),
        };
        let record = |collection: &str, prefix: &str, data: Value| -> artex_core::Record {
            serde_json::from_value(json!({"header":{"id":artex_core::new_id(prefix),"collection":collection,"version":"1","commit_seq":"1","created_at":"2026-10-13T12:34:56Z","updated_at":"2026-10-13T12:34:56Z","legacy_key":null},"data":data})).unwrap()
        };
        let agent = record("agents", "agent", json!({"name":"웹 분석 에이전트"}));
        snapshot
            .collections
            .entry("agents".into())
            .or_default()
            .insert(agent.header.id.clone(), agent.clone());
        let session = record(
            "agent_runs",
            "run",
            json!({"agent_id":agent.header.id,"state":"running"}),
        );
        let approval = record(
            "intercept_pending",
            "intercept",
            json!({"tool_name":"Read","status":"pending"}),
        );
        let session_label = presentation_name(&snapshot, Section::Sessions, &session);
        let approval_label = presentation_name(&snapshot, Section::Approvals, &approval);
        assert!(session_label.contains("웹 분석 에이전트") && session_label.contains("실행 중"));
        assert!(approval_label.contains("Read") && approval_label.contains("승인 대기"));
        assert!(
            !session_label.contains(&session.header.id)
                && !approval_label.contains(&approval.header.id)
        );
        assert_eq!(session.data["state"], "running");
        assert_eq!(approval.data["status"], "pending");
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let label = list_presentation_name(&snapshot, Section::Approvals, &approval);
        assert!(label.contains('\n'));
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_width(218.0);
                object_list_button(ui, false, &label);
            });
        });
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| matches!(&shape.shape, egui::Shape::Text(text)
            if text.galley.text() == label && text.galley.rows.len() >= 2)),
            "narrow list must actually render a separate time line"
        );
    }
    #[test]
    fn conversation_details_start_collapsed_and_user_choices_are_section_local() {
        let mut preferences = BTreeMap::new();
        assert!(!inspector_preference(&preferences, Section::Chat));
        assert!(inspector_preference(&preferences, Section::Tasks));
        preferences.insert(Section::Chat, true);
        preferences.insert(Section::Tasks, false);
        assert!(inspector_preference(&preferences, Section::Chat));
        assert!(!inspector_preference(&preferences, Section::Tasks));
        assert!(inspector_preference(&preferences, Section::Sessions));
        preferences.insert(Section::Chat, false);
        assert!(!inspector_preference(&preferences, Section::Chat));
    }
    #[test]
    fn bypass_is_scoped_persisted_state_and_pending_is_bound_to_open_generation() {
        let mut snapshot = artex_core::Snapshot {
            workspace_id: "workspace-fixture".into(),
            commit_seq: 0,
            collections: Default::default(),
        };
        assert!(!workspace_bypass(&snapshot));
        let header = artex_core::Header {
            id: artex_core::new_id("setting"),
            collection: "settings".into(),
            version: "1".into(),
            commit_seq: "1".into(),
            created_at: artex_core::now(),
            updated_at: artex_core::now(),
            legacy_key: Value::Null,
        };
        snapshot
            .collections
            .entry("settings".into())
            .or_default()
            .insert(
                header.id.clone(),
                artex_core::Record {
                    header: header.clone(),
                    data: json!({"key":"approval_bypass","value":"true"}),
                },
            );
        assert!(workspace_bypass(&snapshot));
        let scope = WorkspaceScope {
            workspace_id: snapshot.workspace_id.clone(),
            engine_epoch: "epoch-fixture".into(),
            open_generation: "open-fixture".into(),
        };
        assert!(bypass_pending(
            &Purpose::BypassToggle {
                scope: scope.clone(),
                enabled: false
            },
            &scope
        ));
        let mut other = scope.clone();
        other.open_generation = "different-open".into();
        assert!(!bypass_pending(
            &Purpose::BypassToggle {
                scope: other,
                enabled: true
            },
            &scope
        ));
        snapshot
            .collections
            .get_mut("settings")
            .unwrap()
            .get_mut(&header.id)
            .unwrap()
            .data["value"] = json!("false");
        assert!(!workspace_bypass(&snapshot));
    }
}

fn section_icon(section: Section) -> Icon {
    match section {
        Section::Overview => Icon::Workspace,
        Section::Tasks | Section::Templates | Section::Categories => Icon::Tasks,
        Section::Chat => Icon::Chat,
        Section::Assets => Icon::Workspace,
        Section::Findings => Icon::Findings,
        Section::Graph => Icon::Graph,
        Section::Traffic => Icon::Traffic,
        Section::Files | Section::Skills => Icon::Files,
        Section::Sessions | Section::Agents => Icon::Agents,
        Section::Tools | Section::Mcp => Icon::Tools,
        Section::Logs | Section::Notifications => Icon::Logs,
        Section::Archives => Icon::Archive,
        _ => Icon::Settings,
    }
}

fn display_state(state: &str) -> &str {
    match state {
        "running" => "실행 중",
        "queued" => "실행 대기",
        "waiting" => "응답 대기",
        "paused" => "일시 정지",
        "completed" | "succeeded" => "완료",
        "failed" => "실패",
        "cancelled" | "canceled" => "취소됨",
        "idle" => "준비",
        "pending" => "승인 대기",
        "blocked" => "차단됨",
        "allowed" => "허용됨",
        "denied" => "거부됨",
        "expired" => "만료됨",
        "stopped" => "중지됨",
        "created" => "준비",
        "interrupted" => "중단됨",
        "recovery_required" => "복구 필요",
        "" => "상태 미기록",
        _ => state,
    }
}

fn navigation_button(ui: &mut egui::Ui, section: Section, active: bool) -> egui::Response {
    let response = ui.add(
        egui::Button::selectable(active, format!("      {}", section.label()))
            .truncate()
            .stroke(egui::Stroke::NONE)
            .corner_radius(8)
            .min_size(egui::vec2(ui.available_width(), 36.0)),
    );
    let rect = egui::Rect::from_min_size(
        response.rect.left_center() + egui::vec2(9.0, -8.0),
        Vec2::splat(16.0),
    );
    let color = if active {
        ui.visuals().selection.stroke.color
    } else {
        theme::muted(ui)
    };
    theme::paint_icon(ui.painter(), rect, section_icon(section), color);
    response
}

impl NativeApp {
    fn details_open(&self) -> bool {
        inspector_preference(&self.inspector_preferences, self.section)
    }
    fn set_details_open(&mut self, open: bool) {
        self.inspector_preferences.insert(self.section, open);
    }
    fn toggle_workspace_bypass(&mut self, view: &NativeReadView) {
        let scope = &view.workspace.scope;
        if self.handle.status().phase != ServicePhase::Ready
            || view.workspace.read_only
            || self.handle.scope().as_ref() != Some(scope)
            || self.pending.values().any(|p| bypass_pending(p, scope))
        {
            return;
        }
        let enabled = !workspace_bypass(&view.generation.snapshot);
        let expected = view
            .generation
            .snapshot
            .records("settings")
            .into_iter()
            .filter(|r| r.data["key"] == "approval_bypass")
            .map(|r| Expected {
                collection: r.header.collection.clone(),
                id: r.header.id.clone(),
                version: r.header.version.clone(),
            })
            .collect();
        // Pin the toolbar's exact generation rather than a previous frame's
        // editor read. The generic prepare/merge/commit CAS path still applies.
        let previous = self.command_read.replace(view.clone());
        self.command(
            "ui.setSettings".into(),
            json!({"patch":{"approval_bypass":enabled}}),
            expected,
            Purpose::BypassToggle {
                scope: scope.clone(),
                enabled,
            },
        );
        self.command_read = previous;
    }
    fn save_chrome_preferences(&mut self, ctx: &egui::Context) {
        if let Err(error) = std::fs::write(
            self.profile.join("native-ui-preferences.json"),
            serde_json::to_vec(&json!({"close_to_tray":self.background,"dark_theme":ctx.theme() == egui::Theme::Dark})).unwrap(),
        ) { self.error = Some(Error::io(error)); }
    }
    fn navigate(&mut self, section: Section) {
        self.navigation_reveal = true;
        if self.section != section {
            self.section = section;
            self.search.clear();
            self.panes.projection.clear();
            if section != Section::Chat && self.drawer.tab == DrawerTab::Management {
                self.drawer.tab = DrawerTab::Activity;
            }
        }
    }

    fn checkpoint_from_chrome(&mut self) {
        if let Some(scope) = self.handle.scope() {
            self.submit(
                NativeAction::Command {
                    scope,
                    command: NativeCommand::checkpoint(),
                },
                Purpose::Checkpoint,
            );
        }
    }

    pub(super) fn draw_chrome(&mut self, ctx: &egui::Context) -> Vec<OperationSubmission> {
        let mut source_submissions = Vec::new();
        if self.section != Section::Chat && self.drawer.tab == DrawerTab::Management {
            self.drawer.tab = DrawerTab::Activity;
        }
        self.list_preferences
            .update_width(ctx.input(|i| i.content_rect().width()));
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            if self.section == Section::Settings {
                self.list_preferences.advanced_settings();
            }
            self.list_preferences.set_open(self.section, true);
        }
        let status = self.handle.status();
        let workspace_choices_enabled = self.workspace_choices_enabled();
        let view = self.handle.view();
        let details_state = view.as_ref().and_then(|view| {
            inspector_item(
                &view.generation.snapshot,
                self.section,
                self.selected.get(&self.section).and_then(Option::as_deref),
            )
            .map(|_| self.details_open())
        });
        let window_titlebar = crate::window_chrome::show_titlebar(
            ctx,
            crate::window_chrome::Platform::current(),
            |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("파일", |ui| {
                        if ui
                            .add_enabled(
                                workspace_choices_enabled,
                                egui::Button::new(format!(
                                    "작업 공간 열기          {}",
                                    shortcut("O")
                                )),
                            )
                            .clicked()
                        {
                            self.choose(false);
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                workspace_choices_enabled,
                                egui::Button::new("읽기 전용으로 열기"),
                            )
                            .clicked()
                        {
                            self.choose_mode(false, true);
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                workspace_choices_enabled,
                                egui::Button::new("새 작업 공간"),
                            )
                            .clicked()
                        {
                            self.choose(true);
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(
                                status.phase == ServicePhase::Ready
                                    && view.as_ref().is_some_and(|v| !v.workspace.read_only),
                                egui::Button::new(format!(
                                    "체크포인트 기록          {}",
                                    shortcut("S")
                                )),
                            )
                            .clicked()
                        {
                            self.checkpoint_from_chrome();
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                status.phase == ServicePhase::Ready,
                                egui::Button::new("저장 후 작업 공간 닫기"),
                            )
                            .clicked()
                        {
                            self.request_close(false);
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .button(format!("앱 종료          {}", shortcut("Q")))
                            .clicked()
                        {
                            self.request_close(true);
                            ui.close();
                        }
                    });
                    ui.menu_button("보기", |ui| {
                        if self.section.collection().is_some() || self.section == Section::Skills {
                            let mut open = self.list_preferences.open(self.section);
                            if ui.checkbox(&mut open, "항목 목록").changed() {
                                if self.section == Section::Settings && open {
                                    self.list_preferences.advanced_settings();
                                }
                                self.list_preferences.set_open(self.section, open);
                            }
                        }
                        if let Some(mut open) = details_state {
                            if ui.checkbox(&mut open, "선택 항목 상세 패널").changed() {
                                self.set_details_open(open);
                            }
                        }
                        if ui
                            .selectable_label(
                                self.drawer.open && self.drawer.tab == DrawerTab::Activity,
                                format!("실행·변경 기록          {}", shortcut("J")),
                            )
                            .clicked()
                        {
                            self.drawer.toggle(DrawerTab::Activity);
                        }
                        if self.section == Section::Chat
                            && ui
                                .add_enabled(
                                    view.is_some(),
                                    egui::Button::selectable(
                                        self.drawer.open
                                            && self.drawer.tab == DrawerTab::Management,
                                        "추가 관리",
                                    ),
                                )
                                .clicked()
                        {
                            self.drawer.toggle(DrawerTab::Management);
                        }
                        ui.separator();
                        for (group, sections) in NAVIGATION_GROUPS {
                            ui.menu_button(*group, |ui| {
                                for section in *sections {
                                    if ui
                                        .selectable_label(self.section == *section, section.label())
                                        .clicked()
                                    {
                                        self.navigate(*section);
                                        ui.close();
                                    }
                                }
                            });
                        }
                    });
                    ui.menu_button("설정", |ui| {
                        for section in [
                            Section::Models,
                            Section::Agents,
                            Section::Tools,
                            Section::Settings,
                        ] {
                            if ui.button(section.label()).clicked() {
                                self.navigate(section);
                                ui.close();
                            }
                        }
                        ui.separator();
                        let mut dark = ctx.theme() == egui::Theme::Dark;
                        if ui.checkbox(&mut dark, "어두운 테마").changed() {
                            ctx.set_theme(if dark {
                                egui::Theme::Dark
                            } else {
                                egui::Theme::Light
                            });
                            theme::apply(ctx);
                            self.save_chrome_preferences(ctx);
                        }
                        if ui
                            .add_enabled(
                                self.tray.available(),
                                egui::Checkbox::new(
                                    &mut self.background,
                                    "창을 닫으면 백그라운드 실행",
                                ),
                            )
                            .changed()
                        {
                            self.save_chrome_preferences(ctx);
                        }
                    });
                    ui.menu_button("도움말", |ui| {
                        if ui.button("ARTEX 정보 · 라이선스          F1").clicked() {
                            self.about_open = true;
                            ui.close();
                        }
                    });
                });
            },
        );
        for action in window_titlebar.actions {
            if action == crate::window_chrome::WindowAction::CloseRequested {
                self.native_window_close(ctx);
            } else {
                crate::window_chrome::dispatch_action(ctx, action);
            }
        }
        if view.is_some() {
            egui::TopBottomPanel::top("desktop_context_toolbar")
            .frame(theme::toolbar_frame(&ctx.style()))
            .show(ctx, |ui| {
                if self.list_preferences.compact {
                    let workspace_name = view.as_ref().map(|view| view.generation.snapshot.records("workspace").first().map(|record| display_name(record)).unwrap_or_else(|| view.workspace.root.file_name().unwrap_or_default().to_string_lossy().into_owned()));
                    let workspace_path = view.as_ref().map(|view| view.workspace.root.to_string_lossy().into_owned());
                    let actions = compact_toolbar(ui, &CompactToolbarPresentation {
                        workspace_name: workspace_name.as_deref(),
                        workspace_path: workspace_path.as_deref(),
                        read_only: view.as_ref().is_some_and(|v|v.workspace.read_only),
                        bypass: view.as_ref().map(|view| (workspace_bypass(&view.generation.snapshot),self.pending.values().any(|p|bypass_pending(p,&view.workspace.scope)),status.phase==ServicePhase::Ready&&!view.workspace.read_only)),
                        list_open: (view.is_some() && (self.section.collection().is_some() || self.section==Section::Skills) && (self.section!=Section::Settings||self.list_preferences.settings_records)).then(||self.list_preferences.open(self.section)),
                        details_open: details_state,
                        workspace_choices_enabled,
                        settings_records: (view.is_some()&&self.section==Section::Settings).then_some(self.list_preferences.settings_records),
                        management_open: (view.is_some()&&self.section==Section::Chat).then_some(self.drawer.open&&self.drawer.tab==DrawerTab::Management),
                    });
                    for action in actions {
                        match action {
                            ChromeAction::Open => self.choose(false),
                            ChromeAction::New => self.choose(true),
                            ChromeAction::ToggleBypass => { if let Some(view)=&view { self.toggle_workspace_bypass(view); } },
                            ChromeAction::List => { let open=self.list_preferences.open(self.section);self.list_preferences.set_open(self.section,!open); },
                            ChromeAction::GeneralSettings => { self.list_preferences.general_settings();self.selected.insert(Section::Settings,None); },
                            ChromeAction::AdvancedSettings => self.list_preferences.advanced_settings(),
                            ChromeAction::Management => self.drawer.toggle(DrawerTab::Management),
                            ChromeAction::Details => self.set_details_open(!self.details_open()),
                        }
                    }
                } else {
                ui.horizontal_wrapped(|ui| {
                    theme::icon(ui, Icon::Workspace, 18.0);
                    for action in workspace_buttons(ui, workspace_choices_enabled) {
                        match action {
                            ChromeAction::Open => self.choose(false),
                            ChromeAction::New => self.choose(true),
                            _ => {}
                        }
                    }
                    ui.separator();
                    ui.strong("ARTEX");
                    if let Some(view) = &view {
                        let name = view
                            .generation
                            .snapshot
                            .records("workspace")
                            .first()
                            .map(|record| display_name(record))
                            .unwrap_or_else(|| {
                                view.workspace
                                    .root
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned()
                            });
                        let name_width = (ui.available_width() * 0.30).clamp(90.0, 260.0);
                        ui.add_sized([name_width, 28.0], egui::Label::new(name).truncate())
                            .on_hover_text(view.workspace.root.to_string_lossy());
                        if view.workspace.read_only {
                            ui.colored_label(
                                theme::palette(ctx.theme() == egui::Theme::Dark).warning,
                                "읽기 전용",
                            );
                        }
                    } else {
                        ui.label("작업 공간 미선택");
                    }
                    if let Some(view) = &view {
                        let enabled = workspace_bypass(&view.generation.snapshot);
                        let pending = self.pending.values().any(|p| bypass_pending(p, &view.workspace.scope));
                        ui.separator();
                        theme::badge(ui,
                            if enabled { "Bypass 켜짐" } else { "Bypass 꺼짐" },
                            if enabled { theme::BadgeTone::Warning } else { theme::BadgeTone::Neutral })
                            .on_hover_text("현재 작업 공간의 내부 실행 승인 생략 상태");
                        if ui.add_enabled(status.phase == ServicePhase::Ready && !view.workspace.read_only && !pending,
                            egui::Button::new(if enabled { "Bypass 끄기" } else { "Bypass 켜기" }))
                            .on_hover_text("현재 작업 공간의 앱 내부 도구 실행 승인을 생략합니다. 읽기 전용·자산 범위·명시적 거부·계획 모드 보호는 유지됩니다.")
                            .clicked() { self.toggle_workspace_bypass(view); }
                        if pending { ui.spinner(); ui.small("적용 중…"); }
                    }
                    if view.is_some() && (self.section.collection().is_some() || self.section == Section::Skills) {
                        ui.separator();
                        if self.section == Section::Settings {
                            if ui.selectable_label(!self.list_preferences.settings_records, "일반 설정").clicked() {
                                self.list_preferences.general_settings();
                                self.selected.insert(Section::Settings, None);
                            }
                            if ui.selectable_label(self.list_preferences.settings_records, "고급 레코드").on_hover_text("내부 저장 레코드를 조회하고 선택합니다. 일반 설정 초안은 유지됩니다.").clicked() {
                                self.list_preferences.advanced_settings();
                            }
                        }
                        if self.section != Section::Settings || self.list_preferences.settings_records {
                            let open = self.list_preferences.open(self.section);
                            if ui.selectable_label(open, if open { "목록 접기" } else { "목록" }).on_hover_text("현재 화면의 항목 선택 · 작은 창에서는 선택 후 자동으로 접습니다").clicked() {
                                self.list_preferences.set_open(self.section, !open);
                            }
                        }
                    }
                    if view.is_some() && self.section == Section::Chat && ui.selectable_label(self.drawer.open && self.drawer.tab == DrawerTab::Management, "추가 관리").clicked() {
                        self.drawer.toggle(DrawerTab::Management);
                    }
                    if let Some(open) = details_state {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(egui::Button::new("상세").selected(open).frame(true))
                                .on_hover_text("선택한 항목의 저장 상태·연결 정보 열기/닫기")
                                .clicked()
                            {
                                self.set_details_open(!open);
                            }
                        });
                    }
                });
                }
            });
        }
        egui::TopBottomPanel::bottom("desktop_status_bar")
            .frame(theme::status_frame(&ctx.style()))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let phase = match status.phase {
                        ServicePhase::Closed => "작업 공간 미선택",
                        ServicePhase::Opening => "작업 공간 여는 중",
                        ServicePhase::Ready => "로컬 작업 공간",
                        ServicePhase::Closing => "저장·종료 중",
                        ServicePhase::Shutdown => "종료",
                        ServicePhase::RecoveryRequired => "복구 필요",
                    };
                    ui.small(phase);
                    if let Some(view) = &view {
                        ui.separator();
                        ui.small(format!("저장 세대 {}", view.generation.commit_seq))
                            .on_hover_text("로컬 저장 기록 번호입니다. 복구·설정·기록 저장도 포함하며 에이전트 실행 횟수가 아닙니다.");
                        let active = view.generation.snapshot.records("agent_runs").iter().filter(|run| {
                            run.data["engine_epoch"] == view.workspace.scope.engine_epoch
                                && matches!(run.data["state"].as_str(), Some("queued" | "running" | "waiting"))
                        }).count();
                        if active > 0 {
                            ui.separator();
                            ui.small(format!("활성 에이전트 {active}"));
                        }
                    }
                    if status.outstanding > 0 {
                        ui.separator();
                        let actions = self.pending.values().filter(|purpose| !background_observation(purpose)).count();
                        if actions > 0 {
                            ui.spinner();
                            ui.small(format!("변경 처리 중 {actions}"));
                        } else {
                            ui.weak(format!("상태 조회 {}", status.outstanding));
                        }
                    }
                    if self.error.is_some()
                        && ui
                            .button(
                                RichText::new("오류 확인")
                                    .color(theme::palette(ctx.theme() == egui::Theme::Dark).error),
                            )
                            .clicked()
                    {
                        self.drawer.show(DrawerTab::Activity);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .selectable_label(
                                self.drawer.open && self.drawer.tab == DrawerTab::Activity,
                                "실행·변경 기록",
                            )
                            .clicked()
                        {
                            self.drawer.toggle(DrawerTab::Activity);
                        }
                    });
                });
            });
        self.drawer.last_rect = None;
        self.drawer.body_rect = None;
        if self.drawer.open {
            let minimum_content = if self.section == Section::Chat {
                self.panes.chat_minimum_height() + 40.0
            } else {
                320.0
            };
            let panel = drawer_panel(ctx, minimum_content, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for tab in [DrawerTab::Activity, DrawerTab::Management] {
                        if tab == DrawerTab::Management && self.section != Section::Chat {
                            continue;
                        }
                        if ui
                            .add_enabled(
                                tab == DrawerTab::Activity || view.is_some(),
                                egui::Button::selectable(self.drawer.tab == tab, tab.label()),
                            )
                            .clicked()
                        {
                            self.drawer.show(tab);
                        }
                    }
                    if self.drawer.tab == DrawerTab::Management {
                        ui.small(self.section.label());
                    }
                    if self.drawer.tab == DrawerTab::Activity {
                        ui.small(format!("대기 {}", status.outstanding));
                    }
                    if self.error.is_some()
                        && self.drawer.tab == DrawerTab::Activity
                        && ui.small_button("오류 닫기").clicked()
                    {
                        self.error = None;
                    }
                    if ui.small_button("접기").clicked() {
                        self.drawer.open = false;
                    }
                });
                let _ = ui.add(egui::Separator::default().spacing(4.0));
                let before_body = ui.available_rect_before_wrap();
                let scroll = drawer_scroll(ui, self.drawer.tab, |ui| match self.drawer.tab {
                    DrawerTab::Activity => {
                        if let Some(error) = &self.error {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("{}: {}", error.code, error.message))
                                        .color(ui.visuals().error_fg_color),
                                )
                                .wrap()
                                .selectable(true),
                            );
                        }
                        for (ticket, purpose) in self.pending.clone() {
                            ui.horizontal_wrapped(|ui| {
                                ui.spinner();
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(purpose_label(&purpose)).small(),
                                    )
                                    .wrap()
                                    .selectable(true),
                                );
                                if ui.small_button("취소").clicked() {
                                    if let Err(error) = self.handle.cancel(&ticket) {
                                        self.error = Some(error);
                                    }
                                }
                            });
                        }
                        if self.error.is_none()
                            && self.pending.is_empty()
                            && self.notices.is_empty()
                        {
                            ui.weak("아직 실행·변경 기록이 없습니다.");
                        }
                        for notice in self.notices.iter().rev().take(40) {
                            ui.add(
                                egui::Label::new(RichText::new(notice).small())
                                    .wrap()
                                    .selectable(true),
                            );
                        }
                    }
                    DrawerTab::Management => {
                        if let Some(view) = &view {
                            source_submissions.extend(self.source_operations.show_expanded(
                                ui,
                                view,
                                self.section,
                                self.selected.get(&self.section).and_then(Option::as_deref),
                            ));
                        } else {
                            ui.weak("작업 공간을 열면 추가 관리를 사용할 수 있습니다.");
                        }
                    }
                });
                let allocated_body = egui::Rect::from_min_max(
                    egui::pos2(scroll.inner_rect.left(), before_body.top()),
                    egui::pos2(
                        scroll.inner_rect.right(),
                        ui.cursor().min.y - ui.spacing().item_spacing.y,
                    ),
                );
                self.drawer.body_rect = Some(allocated_body.intersect(ui.clip_rect()));
            });
            self.drawer.last_rect = Some(panel.response.rect);
        }
        if self.about_open {
            about_window(ctx, &mut self.about_open);
        }
        source_submissions
    }

    pub(super) fn draw_navigation(&mut self, ctx: &egui::Context) {
        let reveal = std::mem::take(&mut self.navigation_reveal);
        egui::SidePanel::left("application_navigation")
            .resizable(true)
            .default_width(190.0)
            .width_range(166.0..=235.0)
            .frame(theme::toolbar_frame(&ctx.style()))
            .show(ctx, |ui| {
                ui.add_space(5.0);
                egui::ScrollArea::vertical()
                    .id_salt("navigation-sections")
                    .show(ui, |ui| {
                        for (index, (group, sections)) in NAVIGATION_GROUPS.iter().enumerate() {
                            if index == 0 {
                                ui.small(*group);
                                ui.add_space(4.0);
                                for section in *sections {
                                    if navigation_button(ui, *section, self.section == *section)
                                        .clicked()
                                    {
                                        self.navigate(*section);
                                    }
                                }
                            } else {
                                ui.add_space(8.0);
                                egui::CollapsingHeader::new(*group)
                                    .id_salt(("navigation-group", index))
                                    .default_open(index < 3 || sections.contains(&self.section))
                                    .open(if reveal && sections.contains(&self.section) {
                                        Some(true)
                                    } else {
                                        None
                                    })
                                    .show(ui, |ui| {
                                        for section in *sections {
                                            if navigation_button(
                                                ui,
                                                *section,
                                                self.section == *section,
                                            )
                                            .clicked()
                                            {
                                                self.navigate(*section);
                                            }
                                        }
                                    });
                            }
                        }
                    });
            });
    }

    pub(super) fn draw_object_list(
        &mut self,
        ctx: &egui::Context,
        snapshot: &artex_core::Snapshot,
        read_only: bool,
    ) {
        self.panes.set_read_only(read_only);
        if (self.section.collection().is_none() && self.section != Section::Skills)
            || !self.list_preferences.open(self.section)
        {
            return;
        }
        egui::SidePanel::left("section_object_list")
            .resizable(true)
            .default_width(218.0)
            .width_range(180.0..=350.0)
            .frame(theme::toolbar_frame(&ctx.style()))
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.strong(if self.section == Section::Settings {
                        "고급 설정 레코드".to_owned()
                    } else {
                        format!("{} 목록", self.section.label())
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!read_only, egui::Button::new("새 항목"))
                            .on_hover_text("새 입력 양식 열기 · 기존 초안은 유지됩니다")
                            .clicked()
                        {
                            self.selected.insert(self.section, None);
                            self.list_preferences.selected(self.section);
                        }
                    });
                });
                ui.add_space(7.0);
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("이름 또는 ID 검색")
                        .desired_width(f32::INFINITY),
                );
                if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
                    search.request_focus();
                }
                ui.add_space(7.0);
                self.panes.object_batch_toolbar(ui, snapshot, self.section);
                ui.separator();
                if self.section == Section::Settings {
                    if let Some(record) = self.selected.get(&Section::Settings).and_then(Option::as_deref).and_then(|id| snapshot.get("settings", id)) {
                        ui.collapsing("선택한 내부 레코드", |ui| {
                            ui.label(display_name(record));
                            ui.add(egui::Label::new(&record.header.id).wrap());
                            ui.small(format!("버전 {}", record.header.version));
                            ui.weak("이 목록은 내부 저장 상태 조회입니다. 일반 설정 양식과 초안은 별도로 유지됩니다.");
                        });
                    }
                }
                egui::ScrollArea::vertical()
                    .id_salt(("workspace-objects", self.section))
                    .show(ui, |ui| {
                        if self.section == Section::Skills {
                            let names: Vec<_> = self
                                .skill_catalog
                                .iter()
                                .filter_map(|skill| skill["name"].as_str())
                                .filter(|name| {
                                    self.search.is_empty()
                                        || name.to_lowercase().contains(&self.search.to_lowercase())
                                })
                                .map(str::to_owned)
                                .collect();
                            ui.small(format!("{}개 항목", names.len()));
                            for name in names {
                                if ui
                                    .add(
                                        egui::Button::selectable(
                                            self.selected
                                                .get(&Section::Skills)
                                                .and_then(Option::as_deref)
                                                == Some(name.as_str()),
                                            &name,
                                        )
                                        .truncate()
                                        .stroke(egui::Stroke::NONE)
                                        .corner_radius(8)
                                        .min_size(egui::vec2(ui.available_width(), 36.0)),
                                    )
                                    .clicked()
                                {
                                    self.selected.insert(Section::Skills, Some(name));
                                    self.list_preferences.selected(self.section);
                                }
                            }
                        } else {
                            let rows = records(snapshot, self.section, &self.search);
                            ui.small(format!("{}개 항목", rows.len()));
                            if rows.is_empty() {
                                ui.add_space(12.0);
                                ui.label("표시할 항목이 없습니다.");
                                if !self.search.is_empty() {
                                    ui.small("검색 조건을 바꿔보세요.");
                                }
                            }
                            for record in rows {
                                let label = if self.section == Section::Models
                                    && record.data["is_default"] == true
                                {
                                    format!("{} · 기본", display_name(record))
                                } else {
                                    list_presentation_name(snapshot, self.section, record)
                                };
                                let selected =
                                    self.selected.get(&self.section).and_then(Option::as_deref)
                                        == Some(record.header.id.as_str());
                                if object_list_row(ui, &mut self.panes, snapshot, self.section, record, selected, &label).clicked() {
                                    self.selected
                                        .insert(self.section, Some(record.header.id.clone()));
                                    self.panes.projection.clear();
                                    self.list_preferences.selected(self.section);
                                }
                            }
                        }
                    });
            });
    }

    pub(super) fn draw_inspector(
        &mut self,
        ctx: &egui::Context,
        snapshot: &artex_core::Snapshot,
        item: Option<&artex_core::Record>,
    ) {
        if !self.details_open() || self.section.configuration() {
            return;
        }
        let Some(item) = item else {
            return;
        };
        let section = self.section;
        let floating = ctx.available_rect().width() < 720.0;
        // Read only: never query, hydrate a form, or change a draft's owner.
        let contents = |ui: &mut egui::Ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.add_space(7.0);
            if !floating {
                ui.strong("선택 항목 상세");
                ui.separator();
            }
            egui::ScrollArea::vertical()
                .id_salt("inspector-content")
                .show(ui, |ui| {
                    ui.add_space(9.0);
                    ui.label(RichText::new(presentation_name(snapshot, section, item)).strong());
                    if let Some(state) = item
                        .data
                        .get("state")
                        .or_else(|| item.data.get("status"))
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        ui.add_space(7.0);
                        ui.small("저장된 상태");
                        ui.label(display_state(state));
                    }
                    for (key, label, collection) in [
                        ("task_id", "연결된 작업", "tasks"),
                        ("agent_id", "참여 에이전트", "agents"),
                        ("company_id", "회사·대상", "companies"),
                        ("asset_id", "참조 자산", "assets"),
                    ] {
                        if let Some(id) = item.data[key].as_str() {
                            ui.add_space(12.0);
                            ui.small(label);
                            ui.label(
                                snapshot
                                    .get(collection, id)
                                    .map(display_name)
                                    .unwrap_or_else(|| "연결된 항목 정보 없음".to_owned()),
                            );
                        }
                    }
                    if matches!(section, Section::Tasks | Section::Chat) {
                        let runs: Vec<_> = snapshot
                            .records("agent_runs")
                            .into_iter()
                            .filter(|run| {
                                if item.header.collection == "tasks" {
                                    run.data["task_id"].as_str() == Some(item.header.id.as_str())
                                } else {
                                    run.data["conversation_id"].as_str()
                                        == Some(item.header.id.as_str())
                                }
                            })
                            .collect();
                        ui.add_space(12.0);
                        ui.small(format!("관련 에이전트 실행 · {}개", runs.len()));
                        ui.horizontal_wrapped(|ui| {
                            let active = runs
                                .iter()
                                .filter(|run| run.data["state"] == "running")
                                .count();
                            let completed = runs
                                .iter()
                                .filter(|run| {
                                    matches!(
                                        run.data["state"].as_str(),
                                        Some("completed" | "succeeded")
                                    )
                                })
                                .count();
                            let waiting = runs
                                .iter()
                                .filter(|run| {
                                    matches!(run.data["state"].as_str(), Some("waiting" | "queued"))
                                })
                                .count();
                            theme::badge(ui, &format!("실행 중 {active}"), theme::BadgeTone::Info);
                            theme::badge(
                                ui,
                                &format!("완료 {completed}"),
                                theme::BadgeTone::Success,
                            );
                            theme::badge(ui, &format!("대기 {waiting}"), theme::BadgeTone::Neutral);
                        });
                        ui.add_space(6.0);
                        for run in runs.iter().take(6) {
                            let agent = run.data["agent_id"]
                                .as_str()
                                .and_then(|id| snapshot.get("agents", id))
                                .map(display_name)
                                .unwrap_or_else(|| {
                                    run.data["agent_key"]
                                        .as_str()
                                        .unwrap_or("에이전트 실행")
                                        .to_owned()
                                });
                            ui.horizontal_wrapped(|ui| {
                                ui.label(agent);
                                ui.small(display_state(run.data["state"].as_str().unwrap_or("")));
                            });
                        }
                    }
                    ui.add_space(15.0);
                    ui.separator();
                    ui.small("현재 저장된 객체");
                    ui.label(format!("버전 {}", item.header.version));
                    ui.small(crate::panes::presentation_time(&item.header.updated_at));
                    ui.collapsing("객체 식별자·원문 상태", |ui| {
                        ui.add(egui::Label::new(&item.header.id).wrap());
                        for key in ["state", "status"] {
                            if let Some(state) = item.data[key].as_str() {
                                ui.label(format!("{key}: {state}"));
                            }
                        }
                        for key in ["agent_id", "task_id", "conversation_id", "tool_use_id"] {
                            if let Some(id) = item.data[key].as_str() {
                                ui.add(egui::Label::new(format!("{key}: {id}")).wrap());
                            }
                        }
                    });
                    ui.add_space(12.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new("편집 중인 초안과 별개의 조회 정보입니다.")
                                .small()
                                .color(theme::muted(ui)),
                        )
                        .wrap(),
                    );
                });
        };
        if !floating {
            egui::SidePanel::right("selection_inspector")
                .resizable(true)
                .default_width(232.0)
                .width_range(205.0..=320.0)
                .frame(theme::toolbar_frame(&ctx.style()))
                .show(ctx, contents);
        } else {
            let mut open = true;
            egui::Window::new("선택 항목 상세")
                .id(egui::Id::new("selection-inspector-window"))
                .open(&mut open)
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 120.0))
                .default_width(280.0)
                .max_width(340.0)
                .max_height((ctx.available_rect().height() - 60.0).max(200.0))
                .show(ctx, contents);
            if !open {
                self.set_details_open(false);
            }
        }
    }
}

#[cfg(test)]
mod details_visibility_tests {
    use super::*;

    fn fixture() -> artex_core::Snapshot {
        let task: artex_core::Record = serde_json::from_value(json!({
            "header":{"id":"task-details","collection":"tasks","version":"1","commit_seq":"1","created_at":"2026-10-15T00:00:00Z","updated_at":"2026-10-15T00:00:00Z","legacy_key":null},
            "data":{"title":"Selected task","status":"created","deleted_at":null,"archived_at":null}
        })).unwrap();
        artex_core::Snapshot {
            workspace_id: "details-fixture".into(),
            commit_seq: 1,
            collections: [(
                "tasks".into(),
                [(task.header.id.clone(), task)].into_iter().collect(),
            )]
            .into_iter()
            .collect(),
        }
    }

    #[test]
    fn only_supported_visible_selection_resolves_for_toolbar_and_inspector() {
        let mut snapshot = fixture();
        assert!(inspector_item(&snapshot, Section::Overview, Some("task-details")).is_none());
        assert!(inspector_item(&snapshot, Section::Settings, Some("task-details")).is_none());
        assert!(inspector_item(&snapshot, Section::Tasks, None).is_none());
        assert!(inspector_item(&snapshot, Section::Tasks, Some("stale-task")).is_none());
        assert!(inspector_item(&snapshot, Section::Tasks, Some("task-details")).is_some());
        assert!(inspector_item(&snapshot, Section::Chat, Some("task-details")).is_some());
        snapshot
            .collections
            .get_mut("tasks")
            .unwrap()
            .get_mut("task-details")
            .unwrap()
            .data["deleted_at"] = json!("2026-10-15T01:00:00Z");
        assert!(inspector_item(&snapshot, Section::Tasks, Some("task-details")).is_none());
        assert!(inspector_item(&snapshot, Section::Chat, Some("task-details")).is_none());
    }

    #[test]
    fn actual_compact_button_is_absent_without_selection_and_emits_toggle_when_clicked() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let render = |details_open, events, time| {
            let mut actions = vec![];
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 620.0),
                    )),
                    events,
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        actions = compact_toolbar(
                            ui,
                            &CompactToolbarPresentation {
                                details_open,
                                ..Default::default()
                            },
                        );
                    });
                },
            );
            (actions, output)
        };
        let (_, hidden) = render(None, vec![], 0.0);
        assert!(
            !hidden.shapes.iter().any(
                |shape| matches!(&shape.shape,egui::Shape::Text(t) if t.galley.text()=="상세")
            )
        );
        let (_, visible) = render(Some(false), vec![], 0.1);
        let pos = visible
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(t) if t.galley.text() == "상세" => {
                    Some(t.pos + t.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("Selected item must expose a details button");
        let pointer = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(
            Some(false),
            vec![egui::Event::PointerMoved(pos), pointer(true)],
            0.2,
        );
        let (actions, _) = render(Some(false), vec![pointer(false)], 0.3);
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, ChromeAction::Details))
        );
    }
}

#[cfg(test)]
mod workspace_button_tests {
    use super::*;

    #[test]
    fn real_toolbar_buttons_emit_open_and_new_and_are_inert_during_transition() {
        for (label, is_open) in [("열기", true), ("새 공간", false)] {
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            let render = |enabled, events, time| {
                let mut actions = vec![];
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(900.0, 620.0),
                        )),
                        events,
                        time: Some(time),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            actions = workspace_buttons(ui, enabled);
                        });
                    },
                );
                (actions, output)
            };
            let (_, frame) = render(true, vec![], 0.0);
            let pos = frame
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(t) if t.galley.text() == label => {
                        Some(t.pos + t.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .unwrap();
            let pointer = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            render(
                true,
                vec![egui::Event::PointerMoved(pos), pointer(true)],
                0.1,
            );
            let (actions, _) = render(true, vec![pointer(false)], 0.2);
            assert_eq!(actions.len(), 1);
            assert!(if is_open {
                matches!(actions[0], ChromeAction::Open)
            } else {
                matches!(actions[0], ChromeAction::New)
            });
            render(
                false,
                vec![egui::Event::PointerMoved(pos), pointer(true)],
                0.4,
            );
            let (actions, _) = render(false, vec![pointer(false)], 0.5);
            assert!(actions.is_empty());
        }
    }
}

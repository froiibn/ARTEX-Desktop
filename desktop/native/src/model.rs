use artex_core::{Expected, Record, Snapshot};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    Overview,
    Tasks,
    Chat,
    Assets,
    Findings,
    Graph,
    Traffic,
    Files,
    Sessions,
    Approvals,
    Models,
    Agents,
    Tools,
    Mcp,
    Skills,
    Constraints,
    Templates,
    Categories,
    Notifications,
    Settings,
    Logs,
    Archives,
    Sync,
}
impl Section {
    pub const ALL: [Self; 23] = [
        Self::Overview,
        Self::Tasks,
        Self::Chat,
        Self::Assets,
        Self::Findings,
        Self::Graph,
        Self::Traffic,
        Self::Files,
        Self::Sessions,
        Self::Approvals,
        Self::Models,
        Self::Agents,
        Self::Tools,
        Self::Mcp,
        Self::Skills,
        Self::Constraints,
        Self::Templates,
        Self::Categories,
        Self::Notifications,
        Self::Settings,
        Self::Logs,
        Self::Archives,
        Self::Sync,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "대시보드",
            Self::Tasks => "작업",
            Self::Chat => "대화",
            Self::Assets => "자산",
            Self::Findings => "발견 사항",
            Self::Graph => "객체 그래프",
            Self::Traffic => "트래픽·증거",
            Self::Files => "작업 파일",
            Self::Sessions => "에이전트 실행",
            Self::Approvals => "실행 승인",
            Self::Models => "LLM 모델",
            Self::Agents => "에이전트",
            Self::Tools => "도구",
            Self::Mcp => "MCP",
            Self::Skills => "Skills",
            Self::Constraints => "작업 제약",
            Self::Templates => "작업 템플릿",
            Self::Categories => "작업 분류",
            Self::Notifications => "알림",
            Self::Settings => "앱 설정",
            Self::Logs => "활동·진단",
            Self::Archives => "보관·복구",
            Self::Sync => "자산 동기화",
        }
    }
    pub fn collection(self) -> Option<&'static str> {
        match self {
            Self::Tasks => Some("tasks"),
            Self::Chat => Some("conversations"),
            Self::Assets => Some("assets"),
            Self::Findings => Some("findings"),
            Self::Traffic => Some("exchanges"),
            Self::Sessions => Some("agent_runs"),
            Self::Approvals => Some("intercept_pending"),
            Self::Models => Some("llm_profiles"),
            Self::Agents => Some("agents"),
            Self::Tools => Some("tools"),
            Self::Mcp => Some("mcp_servers"),
            Self::Constraints => Some("task_constraints"),
            Self::Templates => Some("task_templates"),
            Self::Categories => Some("task_categories"),
            Self::Notifications => Some("notification_channels"),
            Self::Settings => Some("settings"),
            Self::Logs => Some("activity"),
            Self::Archives => Some("task_archives"),
            _ => None,
        }
    }
    pub fn configuration(self) -> bool {
        matches!(
            self,
            Self::Models
                | Self::Agents
                | Self::Tools
                | Self::Mcp
                | Self::Skills
                | Self::Constraints
                | Self::Templates
                | Self::Categories
                | Self::Notifications
                | Self::Settings
        )
    }
}

/// Navigation groups are presentation only; command and selection ownership
/// still use the original Section identity.
pub const NAVIGATION_GROUPS: &[(&str, &[Section])] = &[
    (
        "작업 공간",
        &[Section::Overview, Section::Tasks, Section::Chat],
    ),
    (
        "자산과 증거",
        &[
            Section::Assets,
            Section::Findings,
            Section::Graph,
            Section::Traffic,
            Section::Files,
        ],
    ),
    ("실행", &[Section::Sessions, Section::Approvals]),
    (
        "환경 설정",
        &[
            Section::Models,
            Section::Agents,
            Section::Tools,
            Section::Mcp,
            Section::Skills,
            Section::Constraints,
            Section::Templates,
            Section::Categories,
            Section::Notifications,
            Section::Settings,
        ],
    ),
    ("관리", &[Section::Logs, Section::Archives, Section::Sync]),
];

#[cfg(test)]
mod navigation_tests {
    use super::*;
    #[test]
    fn every_original_section_has_exactly_one_navigation_location() {
        let grouped: Vec<_> = NAVIGATION_GROUPS
            .iter()
            .flat_map(|(_, items)| items.iter().copied())
            .collect();
        let unique: std::collections::BTreeSet<_> = grouped.iter().copied().collect();
        assert_eq!(grouped.len(), Section::ALL.len());
        assert_eq!(unique, Section::ALL.into_iter().collect());
    }
}

pub fn display_name(record: &Record) -> String {
    for field in [
        "title",
        "name",
        "description",
        "key",
        "url",
        "host",
        "summary",
        "kind",
    ] {
        if let Some(text) = record.data[field].as_str().filter(|s| !s.trim().is_empty()) {
            return text
                .lines()
                .next()
                .unwrap_or(text)
                .chars()
                .take(100)
                .collect();
        }
    }
    record.header.id.clone()
}
pub fn text<'a>(data: &'a Value, key: &str) -> &'a str {
    data[key].as_str().unwrap_or("")
}
pub fn expected(record: &Record) -> Expected {
    Expected {
        collection: record.header.collection.clone(),
        id: record.header.id.clone(),
        version: record.header.version.clone(),
    }
}
pub fn selected_record<'a>(
    snapshot: &'a Snapshot,
    section: Section,
    id: Option<&str>,
) -> Option<&'a Record> {
    snapshot
        .get(section.collection()?, id?)
        .filter(|row| row_visible(section, row))
}
fn row_visible(section: Section, row: &Record) -> bool {
    row.data["deleted_at"].is_null()
        && (section != Section::Chat || row.data["trashed_at"].is_null())
        && (section != Section::Tasks || row.data["archived_at"].is_null())
}
pub fn records<'a>(snapshot: &'a Snapshot, section: Section, search: &str) -> Vec<&'a Record> {
    let mut rows = section
        .collection()
        .map(|c| snapshot.records(c))
        .unwrap_or_default();
    let filter = search.trim().to_lowercase();
    rows.retain(|row| {
        row_visible(section, row)
            && (filter.is_empty()
                || display_name(row).to_lowercase().contains(&filter)
                || row.header.id.contains(&filter))
    });
    rows.sort_by(|a, b| {
        b.header
            .updated_at
            .cmp(&a.header.updated_at)
            .then(a.header.id.cmp(&b.header.id))
    });
    rows
}

#[cfg(test)]
mod visibility_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn trash_and_archived_tasks_leave_live_lists_and_restore_the_same_record_identity() {
        let make = |collection: &str, patch: Value| -> Record {
            let id = artex_core::new_id(artex_core::collection_prefix(collection).unwrap());
            let mut data = artex_core::default_data(collection).unwrap();
            for (key, value) in patch.as_object().unwrap() {
                data[key] = value.clone();
            }
            serde_json::from_value(json!({"header":{"id":id,"collection":collection,"version":"7","commit_seq":"9","created_at":"2026-10-14T00:00:00Z","updated_at":"2026-10-14T00:00:00Z","legacy_key":null},"data":data})).unwrap()
        };
        let conversation = make(
            "conversations",
            json!({"title":"Keep history","trashed_at":"2026-10-14T01:00:00Z"}),
        );
        let task = make(
            "tasks",
            json!({"name":"Archived task","archived_at":"2026-10-14T01:00:00Z"}),
        );
        let archive = make(
            "task_archives",
            json!({"state":"ready","task_name":"Archived task"}),
        );
        let mut snapshot = Snapshot {
            workspace_id: artex_core::new_id("workspace"),
            commit_seq: 9,
            collections: std::collections::BTreeMap::new(),
        };
        for row in [conversation.clone(), task.clone(), archive.clone()] {
            snapshot
                .collections
                .entry(row.header.collection.clone())
                .or_default()
                .insert(row.header.id.clone(), row);
        }
        assert!(records(&snapshot, Section::Chat, "").is_empty());
        assert!(selected_record(&snapshot, Section::Chat, Some(&conversation.header.id)).is_none());
        assert!(records(&snapshot, Section::Tasks, "").is_empty());
        assert_eq!(
            records(&snapshot, Section::Archives, "")[0].header.id,
            archive.header.id
        );
        assert!(
            snapshot
                .get("conversations", &conversation.header.id)
                .is_some(),
            "Trash is retained metadata, not a missing hard-deleted record"
        );
        snapshot
            .collections
            .get_mut("conversations")
            .unwrap()
            .get_mut(&conversation.header.id)
            .unwrap()
            .data["trashed_at"] = Value::Null;
        snapshot
            .collections
            .get_mut("tasks")
            .unwrap()
            .get_mut(&task.header.id)
            .unwrap()
            .data["archived_at"] = Value::Null;
        assert_eq!(
            records(&snapshot, Section::Chat, "")[0].header.id,
            conversation.header.id
        );
        assert_eq!(
            selected_record(&snapshot, Section::Tasks, Some(&task.header.id))
                .unwrap()
                .header
                .created_at,
            task.header.created_at
        );
    }
}

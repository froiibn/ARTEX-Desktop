//! Queue grouping follows the original serial/all/by_task/none scheduler.
use crate::*;
#[derive(Clone, Debug)]
pub struct Fire {
    pub delivery_id: String,
    pub task_id: Option<String>,
    pub mergeable: bool,
    pub description: String,
    pub goal: String,
    pub message: String,
}
pub fn take_group(agent: &Value, queue: &[Fire]) -> Vec<Fire> {
    let Some(head) = queue.first() else {
        return vec![];
    };
    if agent["trigger_run_mode"] == "parallel" || agent["trigger_merge_mode"] == "none" {
        return vec![head.clone()];
    }
    if agent["trigger_merge_mode"] == "by_task" {
        if !head.mergeable || head.task_id.is_none() {
            return vec![head.clone()];
        }
        return queue
            .iter()
            .filter(|f| f.mergeable && f.task_id == head.task_id)
            .cloned()
            .collect();
    }
    queue.to_vec()
}
fn header(f: &Fire) -> String {
    if f.description.is_empty() && f.goal.is_empty() {
        return String::new();
    }
    format!(
        "[Task {} {}{}]",
        f.task_id.as_deref().unwrap_or("none"),
        f.description.chars().take(200).collect::<String>(),
        if f.goal.is_empty() {
            String::new()
        } else {
            format!(" (goal: {})", f.goal.chars().take(500).collect::<String>())
        }
    )
}
pub fn message(items: &[Fire]) -> String {
    if items.len() == 1 {
        let f = &items[0];
        let h = header(f);
        return if h.is_empty() {
            f.message.clone()
        } else {
            format!("{h}\n{}", f.message)
        };
    }
    let mut order = vec![];
    for f in items {
        if !order.contains(&f.task_id) {
            order.push(f.task_id.clone());
        }
    }
    let mut text = format!(
        "This session merges {} queued trigger events across {} task groups. Handle the events together.\n",
        items.len(),
        order.len()
    );
    let mut n = 0;
    for task in order {
        let group = items
            .iter()
            .filter(|f| f.task_id == task)
            .collect::<Vec<_>>();
        let h = header(group[0]);
        if !h.is_empty() {
            text.push_str(&format!("\n{h}\n"));
        }
        for f in group {
            n += 1;
            text.push_str(&format!("\n-- trigger {n} --\n{}\n", f.message));
        }
    }
    text
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fire(task: &str, body: &str) -> Fire {
        Fire {
            delivery_id: body.into(),
            task_id: Some(task.into()),
            mergeable: true,
            description: task.into(),
            goal: format!("goal-{task}"),
            message: body.into(),
        }
    }
    #[test]
    fn interleaved_tasks_header_once_and_serial_modes() {
        let q = vec![fire("A", "1"), fire("B", "2"), fire("A", "3")];
        let all = take_group(&json!({"trigger_merge_mode":"all"}), &q);
        let m = message(&all);
        assert_eq!(m.matches("goal-A").count(), 1);
        assert_eq!(m.matches("goal-B").count(), 1);
        assert!(m.find("\n3\n").unwrap() < m.find("\n2\n").unwrap());
        let by = take_group(&json!({"trigger_merge_mode":"by_task"}), &q);
        assert_eq!(by.len(), 2);
        assert_eq!(
            take_group(&json!({"trigger_merge_mode":"none"}), &q).len(),
            1
        );
        assert_eq!(
            take_group(
                &json!({"trigger_merge_mode":"all","trigger_run_mode":"parallel"}),
                &q
            )
            .len(),
            1
        );
        let mut q = q;
        q[0].mergeable = false;
        assert_eq!(
            take_group(&json!({"trigger_merge_mode":"by_task"}), &q).len(),
            1
        );
    }
}

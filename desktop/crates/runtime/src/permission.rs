use serde_json::Value;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}
fn matches(rule: &str, name: &str) -> bool {
    rule == "*" || rule.split('(').next().unwrap_or(rule) == name
}
pub fn decide(
    mode: &str,
    name: &str,
    read_only: bool,
    allow: &[String],
    deny: &[String],
) -> Decision {
    if deny.iter().any(|r| matches(r, name)) {
        return Decision::Deny;
    }
    if mode == "bypassPermissions" {
        return Decision::Allow;
    }
    if mode == "plan" && !read_only {
        return Decision::Deny;
    }
    if read_only
        || allow.iter().any(|r| matches(r, name))
        || mode == "acceptEdits" && ["Write", "Edit", "MultiEdit"].contains(&name)
    {
        return Decision::Allow;
    }
    if ["dontAsk", "auto", "plan"].contains(&mode) {
        Decision::Deny
    } else {
        Decision::Ask
    }
}
pub fn rules(value: &Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deny_precedes_bypass_and_plan_never_writes() {
        assert_eq!(
            decide(
                "bypassPermissions",
                "Bash",
                false,
                &[],
                &["Bash(command)".into()]
            ),
            Decision::Deny
        );
        assert_eq!(
            decide("plan", "Write", false, &["Write".into()], &[]),
            Decision::Deny
        );
        assert_eq!(decide("default", "Read", true, &[], &[]), Decision::Allow);
        assert_eq!(decide("default", "Bash", false, &[], &[]), Decision::Ask);
        assert_eq!(decide("dontAsk", "Bash", false, &[], &[]), Decision::Deny);
    }
}

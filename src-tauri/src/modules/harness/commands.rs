//! Tauri command glue for the harness layer, deliberately thin.

use serde::Serialize;

use super::Caps;

/// One Harness as the frontend sees it: identity plus its Cap set, so the
/// UI can switch surfaces off per missing Cap.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessInfo {
    pub id: &'static str,
    pub display_name: &'static str,
    pub caps: Caps,
}

#[tauri::command]
pub fn helm_list_harnesses() -> Vec<HarnessInfo> {
    super::all()
        .iter()
        .map(|h| HarnessInfo {
            id: h.id(),
            display_name: h.display_name(),
            caps: h.caps(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_claude_code_with_its_caps() {
        let harnesses = helm_list_harnesses();
        assert_eq!(harnesses.len(), 1);
        assert_eq!(harnesses[0].id, "claude-code");
        assert_eq!(harnesses[0].display_name, "Claude Code");
        assert_eq!(harnesses[0].caps, crate::modules::harness::claude_code::CLAUDE_CODE_CAPS);
    }
}

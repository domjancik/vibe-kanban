use executors::model_selector::ModelInfo;
use ratatui::layout::Rect;

pub(crate) fn default_variant_to_none(variant: String) -> Option<String> {
    if variant == "DEFAULT" {
        None
    } else {
        Some(variant)
    }
}

pub(crate) fn model_key(model: &ModelInfo) -> String {
    if let Some(provider_id) = &model.provider_id {
        format!("{provider_id}/{}", model.id)
    } else {
        model.id.clone()
    }
}

pub(crate) fn fuzzy_contains(query: &str, candidate: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let candidate = candidate.to_lowercase();
    if candidate.contains(&query) {
        return true;
    }

    let mut query_chars = query.chars();
    let mut current = query_chars.next();
    for ch in candidate.chars() {
        if current.is_some_and(|needle| needle == ch) {
            current = query_chars.next();
            if current.is_none() {
                return true;
            }
        }
    }
    false
}

pub(crate) fn viewport_capacity(area: Rect, rows_per_item: u16) -> usize {
    area.height
        .saturating_sub(2)
        .max(1)
        .div_ceil(rows_per_item)
        .max(1) as usize
}

pub(crate) fn selected_list_offset(selected: usize, total: usize, viewport: usize) -> usize {
    if total <= viewport {
        0
    } else {
        selected
            .saturating_sub(viewport.saturating_sub(1))
            .min(total.saturating_sub(viewport))
    }
}

#[cfg(test)]
mod tests {
    use executors::model_selector::ModelInfo;
    use ratatui::layout::Rect;

    use super::{
        default_variant_to_none, fuzzy_contains, model_key, selected_list_offset, viewport_capacity,
    };

    #[test]
    fn fuzzy_contains_supports_substring_and_sparse_matching() {
        assert!(fuzzy_contains("agent", "Coding Agent"));
        assert!(fuzzy_contains("cda", "Codex Default Agent"));
        assert!(!fuzzy_contains("azc", "Codex Default Agent"));
    }

    #[test]
    fn variant_and_model_helpers_normalize_expected_values() {
        assert_eq!(default_variant_to_none("DEFAULT".to_string()), None);
        assert_eq!(
            default_variant_to_none("PLAN".to_string()),
            Some("PLAN".to_string())
        );

        let model = ModelInfo {
            id: "gpt-5".to_string(),
            name: "GPT-5".to_string(),
            provider_id: Some("openai".to_string()),
            reasoning_options: Vec::new(),
        };
        assert_eq!(model_key(&model), "openai/gpt-5");
    }

    #[test]
    fn viewport_helpers_keep_selection_visible() {
        assert_eq!(viewport_capacity(Rect::new(0, 0, 20, 2), 3), 1);
        assert_eq!(viewport_capacity(Rect::new(0, 0, 20, 8), 2), 3);

        assert_eq!(selected_list_offset(1, 3, 5), 0);
        assert_eq!(selected_list_offset(4, 10, 3), 2);
        assert_eq!(selected_list_offset(9, 10, 3), 7);
    }
}

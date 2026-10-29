//! The data of the model picker: the groups of models by account, the search
//! filter, the effort levels of a model, and the rules that make a new
//! selection when the user chooses a model or an effort. Every function is
//! pure, so each client draws the same picker from the same view state.

use consort_proto::model::{AuthState, HarnessAccount, OptionDescriptor};

use crate::view::{
    EffectiveSelection, EffortLevel, EffortView, ModelChoiceView, ModelGroupView, ProviderLogo,
    SelectionOption, SelectionSource, SelectionView,
};

/// The option ID of the effort in a model selection.
pub const EFFORT: &str = "effort";

/// The value that some providers list among the effort levels for "let the
/// model decide". It is not a level: a selection without an effort gets the
/// same.
const DEFAULT_VALUE: &str = "default";

/// The text of an effort level. Providers send short words, such as
/// "Xhigh" for `xhigh`, so the picker says "Extra high", and a lowercase
/// word gets a capital.
pub fn level_label(value: &str, label: &str) -> String {
    if value.eq_ignore_ascii_case("xhigh") && label.eq_ignore_ascii_case("xhigh") {
        return "Extra high".to_owned();
    }
    let label = if label.trim().is_empty() {
        value
    } else {
        label
    };
    let mut chars = label.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The effort levels in a model's options. `None` when the model has no
/// effort option, or when it lists no level.
pub fn effort_view(options: &[OptionDescriptor]) -> Option<EffortView> {
    let (choices, default) = options.iter().find_map(|o| match o {
        OptionDescriptor::Select {
            id,
            choices,
            default,
            ..
        } if id == EFFORT => Some((choices, default)),
        _ => None,
    })?;
    let levels: Vec<EffortLevel> = choices
        .iter()
        .filter(|c| c.value != DEFAULT_VALUE)
        .map(|c| EffortLevel {
            value: c.value.clone(),
            label: level_label(&c.value, &c.label),
        })
        .collect();
    if levels.is_empty() {
        return None;
    }
    let default = default
        .as_ref()
        .filter(|d| levels.iter().any(|l| &l.value == *d))
        .cloned();
    Some(EffortView { levels, default })
}

/// The picker groups from the accounts of each worker, by worker name. One
/// group per account label, in the order that the labels first appear. Only
/// accounts that are enabled and signed in count. A group lists the workers
/// where its account is signed in, and the union of their models.
/// `provider` gives the provider name and logo of a harness.
pub fn model_groups<'a>(
    workers: impl IntoIterator<Item = (&'a str, &'a [HarnessAccount])>,
    provider: impl Fn(&str) -> (String, ProviderLogo),
) -> Vec<ModelGroupView> {
    let mut groups: Vec<ModelGroupView> = Vec::new();
    for (worker, accounts) in workers {
        for account in accounts
            .iter()
            .filter(|a| a.enabled && a.auth_state == AuthState::SignedIn)
        {
            let index = match groups.iter().position(|g| g.account_label == account.label) {
                Some(index) => index,
                None => {
                    let (name, logo) = provider(&account.harness);
                    groups.push(ModelGroupView {
                        account_label: account.label.clone(),
                        provider: name,
                        logo,
                        workers: Vec::new(),
                        models: Vec::new(),
                    });
                    groups.len() - 1
                }
            };
            let group = &mut groups[index];
            if !group.workers.iter().any(|w| w == worker) {
                group.workers.push(worker.to_owned());
            }
            for model in &account.models {
                if !group.models.iter().any(|m| m.id == model.id) {
                    group.models.push(ModelChoiceView {
                        id: model.id.clone(),
                        name: model.name.clone(),
                        effort: effort_view(&model.options),
                    });
                }
            }
        }
    }
    groups
}

/// The groups and models that match a search. Each word of the query must
/// be in the model name, the model ID, the account name or the provider
/// name, in any case. A group with no match is left out. An empty query
/// keeps everything.
pub fn filter(groups: &[ModelGroupView], query: &str) -> Vec<ModelGroupView> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return groups.to_vec();
    }
    groups
        .iter()
        .filter_map(|group| {
            let models: Vec<ModelChoiceView> = group
                .models
                .iter()
                .filter(|model| {
                    let text = format!(
                        "{}\n{}\n{}\n{}",
                        model.name, model.id, group.account_label, group.provider
                    )
                    .to_lowercase();
                    words.iter().all(|w| text.contains(w.as_str()))
                })
                .cloned()
                .collect();
            (!models.is_empty()).then(|| ModelGroupView {
                models,
                ..group.clone()
            })
        })
        .collect()
}

/// One row of the effort menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffortRow {
    /// The value to choose. `None` for the row that clears the effort, so
    /// the provider uses its default.
    pub value: Option<String>,
    pub label: String,
    /// The provider uses this level when the selection has none.
    pub is_default: bool,
    pub selected: bool,
}

impl EffortView {
    /// Whether the model offers this level.
    pub fn offers(&self, value: &str) -> bool {
        self.levels.iter().any(|l| l.value == value)
    }

    /// The text of a level, or `None` for a level the model does not offer.
    pub fn label_of(&self, value: &str) -> Option<&str> {
        self.levels
            .iter()
            .find(|l| l.value == value)
            .map(|l| l.label.as_str())
    }

    /// The range for a picker row, from the first and the last level, such as
    /// "Low to Max effort", or "High effort" for one level.
    pub fn range_label(&self) -> String {
        match self.levels.as_slice() {
            [] => String::new(),
            [only] => format!("{} effort", only.label),
            [first, .., last] => format!("{} to {} effort", first.label, last.label),
        }
    }

    /// The rows of the effort menu, for a selection whose effort is
    /// `chosen`. When the provider does not say which level is its default,
    /// a first row "Default" clears the effort. A chosen value that the
    /// model does not offer counts as no choice.
    pub fn rows(&self, chosen: Option<&str>) -> Vec<EffortRow> {
        let chosen = chosen.filter(|v| self.offers(v));
        let mut rows = Vec::new();
        if self.default.is_none() {
            rows.push(EffortRow {
                value: None,
                label: "Default".to_owned(),
                is_default: false,
                selected: chosen.is_none(),
            });
        }
        let effective = chosen.or(self.default.as_deref());
        rows.extend(self.levels.iter().map(|l| EffortRow {
            value: Some(l.value.clone()),
            label: l.label.clone(),
            is_default: self.default.as_deref() == Some(l.value.as_str()),
            selected: effective == Some(l.value.as_str()),
        }));
        rows
    }

    /// The text of the effort chip: the chosen level, else the provider's
    /// default level, else "Default".
    pub fn chip_label(&self, chosen: Option<&str>) -> String {
        chosen
            .and_then(|v| self.label_of(v))
            .or_else(|| self.default.as_deref().and_then(|d| self.label_of(d)))
            .unwrap_or("Default")
            .to_owned()
    }
}

/// The selection after the user chooses `model` of `group`. The effort
/// stays when the new model offers that level. Otherwise the selection has
/// no effort, and the provider uses the new model's default. Other options
/// stay only on the same model.
pub fn choose_model(
    previous: Option<&SelectionView>,
    group: &ModelGroupView,
    model: &ModelChoiceView,
) -> SelectionView {
    let same_model =
        previous.is_some_and(|p| p.account_label == group.account_label && p.model_id == model.id);
    let mut options: Vec<(String, SelectionOption)> = if same_model {
        previous.map(|p| p.options.clone()).unwrap_or_default()
    } else {
        Vec::new()
    };
    options.retain(|(id, _)| id != EFFORT);
    let kept = previous
        .and_then(SelectionView::effort)
        .filter(|v| model.effort.as_ref().is_some_and(|e| e.offers(v)));
    if let Some(value) = kept {
        options.push((EFFORT.to_owned(), SelectionOption::Text(value.to_owned())));
    }
    SelectionView {
        account_label: group.account_label.clone(),
        model_id: model.id.clone(),
        model_name: model.name.clone(),
        options,
    }
}

/// The selection with a new effort. `None` takes the effort out.
pub fn with_effort(selection: &SelectionView, value: Option<&str>) -> SelectionView {
    let mut out = selection.clone();
    out.options.retain(|(id, _)| id != EFFORT);
    if let Some(value) = value {
        out.options
            .push((EFFORT.to_owned(), SelectionOption::Text(value.to_owned())));
    }
    out
}

/// The channel's own coordinator selection, else Everything's.
pub fn effective_selection(
    own: Option<SelectionView>,
    everything: Option<SelectionView>,
) -> Option<EffectiveSelection> {
    match (own, everything) {
        (Some(selection), _) => Some(EffectiveSelection {
            selection,
            source: SelectionSource::Channel,
        }),
        (None, Some(selection)) => Some(EffectiveSelection {
            selection,
            source: SelectionSource::Everything,
        }),
        (None, None) => None,
    }
}

/// What a choice in the picker changes. The picker says it in its footer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerPurpose {
    /// The assistant in Everything, and the default for every channel.
    Everything,
    /// The assistant in the channel with this name.
    Channel(String),
    /// The tasks of the channel with this name.
    Tasks(String),
}

impl PickerPurpose {
    /// The footer line of the picker.
    pub fn footer(&self) -> String {
        match self {
            PickerPurpose::Everything => "The assistant in Everything uses this model. Channels without their own model use it too.".to_owned(),
            PickerPurpose::Channel(name) => format!("The assistant in {} uses this model.", channel_words(name)),
            PickerPurpose::Tasks(name) => format!("Tasks in {} start with this model.", channel_words(name)),
        }
    }
}

/// A channel name in a sentence. A channel with no name yet, in the New
/// channel dialog, is "this channel".
fn channel_words(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        "this channel".to_owned()
    } else {
        name.to_owned()
    }
}

/// A selection in words, such as "Opus 5.5, Claude Personal, High effort".
/// The effort is left out when the selection has none.
pub fn selection_words(selection: &SelectionView, effort: Option<&EffortView>) -> String {
    let mut parts = Vec::new();
    if !selection.model_id.is_empty() {
        parts.push(selection.model_name.clone());
    }
    parts.push(selection.account_label.clone());
    if let Some(value) = selection.effort() {
        let label = effort
            .and_then(|e| e.label_of(value))
            .map(str::to_owned)
            .unwrap_or_else(|| level_label(value, value));
        parts.push(format!("{label} effort"));
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use consort_proto::ids::AccountId;
    use consort_proto::model::{Capabilities, ModelInfo, OptionChoice};

    use super::*;

    fn effort(levels: &[&str], default: Option<&str>) -> Vec<OptionDescriptor> {
        vec![OptionDescriptor::Select {
            id: EFFORT.to_owned(),
            label: "Effort".to_owned(),
            choices: levels
                .iter()
                .map(|v| OptionChoice {
                    value: (*v).to_owned(),
                    label: v.to_uppercase(),
                })
                .collect(),
            default: default.map(str::to_owned),
        }]
    }

    fn model(id: &str, options: Vec<OptionDescriptor>) -> ModelInfo {
        ModelInfo {
            id: id.to_owned(),
            name: format!("Model {id}"),
            options,
        }
    }

    fn account(
        label: &str,
        harness: &str,
        auth: AuthState,
        models: Vec<ModelInfo>,
    ) -> HarnessAccount {
        HarnessAccount {
            account_id: AccountId::new(),
            harness: harness.to_owned(),
            label: label.to_owned(),
            harness_version: None,
            auth_state: auth,
            auth_detail: None,
            models,
            capabilities: Capabilities::default(),
            enabled: true,
            profile: true,
            email: None,
            plan: None,
            secret_names: Vec::new(),
            sign_in_methods: Vec::new(),
        }
    }

    fn groups() -> Vec<ModelGroupView> {
        let laptop = vec![
            account(
                "Claude Work",
                "claude",
                AuthState::SignedIn,
                vec![model("opus", effort(&["low", "high", "max"], Some("high")))],
            ),
            account(
                "Claude Personal",
                "claude",
                AuthState::SignedOut,
                vec![model("opus", Vec::new())],
            ),
        ];
        let desktop = vec![
            account(
                "Codex Personal",
                "codex",
                AuthState::SignedIn,
                vec![model("gpt", effort(&["low", "xhigh"], None))],
            ),
            account(
                "Claude Work",
                "claude",
                AuthState::SignedIn,
                vec![model("opus", Vec::new()), model("haiku", Vec::new())],
            ),
        ];
        model_groups(
            [
                ("work-laptop", laptop.as_slice()),
                ("work-desktop", desktop.as_slice()),
            ],
            crate::view::provider_of,
        )
    }

    #[test]
    fn models_group_by_account_label_across_workers() {
        let groups = groups();
        let labels: Vec<&str> = groups.iter().map(|g| g.account_label.as_str()).collect();
        assert_eq!(labels, ["Claude Work", "Codex Personal"]);
        let work = &groups[0];
        assert_eq!(work.workers, ["work-laptop", "work-desktop"]);
        assert_eq!(work.provider, "Claude Code");
        assert_eq!(work.logo, ProviderLogo::ClaudeCode);
        // The union, in the order the workers report them. The first
        // worker's options win for a model that both report.
        let ids: Vec<&str> = work.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["opus", "haiku"]);
        assert!(work.models[0].effort.is_some());
    }

    #[test]
    fn a_signed_out_or_disabled_account_is_left_out() {
        let groups = groups();
        assert!(groups.iter().all(|g| g.account_label != "Claude Personal"));
        let mut off = account(
            "Off",
            "claude",
            AuthState::SignedIn,
            vec![model("a", Vec::new())],
        );
        off.enabled = false;
        let accounts = [off];
        assert!(model_groups([("w", accounts.as_slice())], crate::view::provider_of).is_empty());
    }

    #[test]
    fn search_matches_model_name_id_account_and_provider() {
        let groups = groups();
        let ids = |found: Vec<ModelGroupView>| -> Vec<String> {
            found
                .iter()
                .flat_map(|g| {
                    g.models
                        .iter()
                        .map(|m| format!("{}/{}", g.account_label, m.id))
                })
                .collect()
        };
        assert_eq!(ids(filter(&groups, "HAIKU")), ["Claude Work/haiku"]);
        assert_eq!(ids(filter(&groups, "model gpt")), ["Codex Personal/gpt"]);
        assert_eq!(
            ids(filter(&groups, "work")),
            ["Claude Work/opus", "Claude Work/haiku"],
            "the account name matches every model of the account"
        );
        assert_eq!(ids(filter(&groups, "codex")), ["Codex Personal/gpt"]);
        assert_eq!(
            ids(filter(&groups, "claude code opus")),
            ["Claude Work/opus"]
        );
        assert!(filter(&groups, "nothing like this").is_empty());
        assert_eq!(filter(&groups, "  "), groups);
    }

    #[test]
    fn effort_levels_leave_out_the_default_value_and_say_extra_high() {
        let view = effort_view(&effort(
            &["default", "low", "xhigh", "max"],
            Some("default"),
        ))
        .unwrap();
        let values: Vec<&str> = view.levels.iter().map(|l| l.value.as_str()).collect();
        assert_eq!(values, ["low", "xhigh", "max"]);
        assert_eq!(view.levels[1].label, "Extra high");
        assert_eq!(view.default, None, "`default` is not a level");
        assert_eq!(view.range_label(), "LOW to MAX effort");
        assert!(effort_view(&effort(&["default"], None)).is_none());
        assert!(effort_view(&[]).is_none());
        assert_eq!(level_label("medium", "medium"), "Medium");
    }

    #[test]
    fn the_effort_menu_marks_the_default_and_the_choice() {
        let known = effort_view(&effort(&["low", "medium", "high"], Some("medium"))).unwrap();
        let rows = known.rows(None);
        assert_eq!(
            rows.len(),
            3,
            "no Default row when the provider names its default"
        );
        assert!(rows[1].is_default && rows[1].selected);
        let rows = known.rows(Some("high"));
        assert!(rows[2].selected && !rows[1].selected);
        assert_eq!(known.chip_label(None), "MEDIUM");
        assert_eq!(known.chip_label(Some("high")), "HIGH");

        let unknown = effort_view(&effort(&["low", "high"], None)).unwrap();
        let rows = unknown.rows(None);
        assert_eq!(rows[0].value, None);
        assert_eq!(rows[0].label, "Default");
        assert!(rows[0].selected);
        assert_eq!(unknown.chip_label(None), "Default");
        // A value that the model does not offer counts as no choice.
        assert!(unknown.rows(Some("max"))[0].selected);
    }

    #[test]
    fn a_new_model_keeps_the_effort_only_when_it_offers_that_level() {
        let groups = groups();
        let (work, codex) = (&groups[0], &groups[1]);
        let opus = &work.models[0];
        let first = choose_model(None, work, opus);
        assert!(
            first.options.is_empty(),
            "a first choice has the provider default"
        );
        let high = with_effort(&first, Some("high"));
        assert_eq!(high.effort(), Some("high"));
        // GPT offers low and xhigh, not high: the provider default.
        let gpt = choose_model(Some(&high), codex, &codex.models[0]);
        assert_eq!(gpt.effort(), None);
        assert_eq!(gpt.account_label, "Codex Personal");
        // Low is offered by both, so it stays.
        let low = with_effort(&high, Some("low"));
        assert_eq!(
            choose_model(Some(&low), codex, &codex.models[0]).effort(),
            Some("low")
        );
        // Haiku has no effort option at all.
        assert_eq!(
            choose_model(Some(&low), work, &work.models[1]).effort(),
            None
        );
        assert_eq!(with_effort(&low, None).effort(), None);
    }

    #[test]
    fn the_effective_selection_says_where_it_comes_from() {
        let sel = |m: &str| SelectionView {
            account_label: "A".into(),
            model_id: m.into(),
            model_name: m.into(),
            options: Vec::new(),
        };
        let own = effective_selection(Some(sel("own")), Some(sel("all"))).unwrap();
        assert_eq!(
            (own.selection.model_id.as_str(), own.source),
            ("own", SelectionSource::Channel)
        );
        let inherited = effective_selection(None, Some(sel("all"))).unwrap();
        assert_eq!(
            (inherited.selection.model_id.as_str(), inherited.source),
            ("all", SelectionSource::Everything)
        );
        assert_eq!(effective_selection(None, None), None);
    }

    #[test]
    fn the_footer_says_what_a_choice_changes() {
        assert!(
            PickerPurpose::Everything
                .footer()
                .contains("Channels without their own model")
        );
        assert_eq!(
            PickerPurpose::Channel("sayso".into()).footer(),
            "The assistant in sayso uses this model."
        );
        assert_eq!(
            PickerPurpose::Tasks(" ".into()).footer(),
            "Tasks in this channel start with this model."
        );
    }

    #[test]
    fn a_selection_in_words() {
        let groups = groups();
        let work = &groups[0];
        let sel = with_effort(&choose_model(None, work, &work.models[0]), Some("high"));
        assert_eq!(
            selection_words(&sel, work.models[0].effort.as_ref()),
            "Model opus, Claude Work, HIGH effort"
        );
        assert_eq!(
            selection_words(&sel, None),
            "Model opus, Claude Work, High effort"
        );
    }
}

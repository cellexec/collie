//! collie: legt `collie_defaults.toml` als Layer unter die Nutzer-Config.
//!
//! Die herdr-Defaults in den `Default`-Impls bleiben unverändert, damit
//! Upstream-Merges sauber bleiben. Die Loader in `io.rs` reichen den Text der
//! Nutzer-Config durch [`layer_under`], bevor sie ihn parsen.

const COLLIE_DEFAULTS: &str = include_str!("collie_defaults.toml");
/// `HERDR_COLLIE_DEFAULTS=0` schaltet den Layer ab. Die Integrationstests
/// setzen das, damit sie weiter gegen die herdr-Defaults laufen.
const DISABLE_ENV_VAR: &str = "HERDR_COLLIE_DEFAULTS";

/// Liefert den Config-Text, den die Loader parsen: die collie-Defaults mit der
/// Nutzer-Config darüber. Ohne Nutzer-Config sind es die collie-Defaults allein.
/// Lässt sich die Nutzer-Config nicht als TOML lesen, kommt sie unverändert
/// zurück, damit die Parse-Fehler der Loader auf ihren Text zeigen.
pub(super) fn layer_under(user: Option<&str>) -> String {
    if std::env::var(DISABLE_ENV_VAR).is_ok_and(|value| value == "0") {
        return user.unwrap_or_default().to_owned();
    }
    layer(user)
}

fn layer(user: Option<&str>) -> String {
    let Ok(toml::Value::Table(mut merged)) = COLLIE_DEFAULTS.parse::<toml::Value>() else {
        tracing::warn!("collie defaults are not a TOML table, skipping them");
        return user.unwrap_or_default().to_owned();
    };
    let Some(user) = user else {
        return COLLIE_DEFAULTS.to_owned();
    };
    let Ok(toml::Value::Table(user_table)) = user.parse::<toml::Value>() else {
        return user.to_owned();
    };
    merge_tables(&mut merged, user_table);
    match toml::to_string(&toml::Value::Table(merged)) {
        Ok(content) => content,
        Err(err) => {
            tracing::warn!(err = %err, "could not layer collie defaults, using user config only");
            user.to_owned()
        }
    }
}

fn merge_tables(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(base_table)), toml::Value::Table(over_table)) => {
                merge_tables(base_table, over_table);
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, StatusIndicatorStyle};

    fn parse(content: &str) -> Config {
        toml::from_str(content).unwrap()
    }

    #[test]
    fn defaults_parse_without_unknown_keys() {
        let mut ignored = Vec::new();
        let config: Config =
            serde_ignored::deserialize(toml::Deserializer::new(COLLIE_DEFAULTS), |path| {
                ignored.push(path.to_string())
            })
            .unwrap();
        assert!(ignored.is_empty(), "unknown keys: {ignored:?}");
        assert!(config.collect_diagnostics().is_empty());
    }

    #[test]
    fn missing_user_config_yields_collie_defaults() {
        let config = parse(&layer(None));
        assert!(config.ui.sidebar.agents_only);
        assert!(config.ui.sidebar.overlay);
        assert_eq!(config.ui.status_indicators, StatusIndicatorStyle::Symbols);
    }

    #[test]
    fn user_values_win_and_siblings_keep_collie_defaults() {
        let config = parse(&layer(Some(
            "[ui.sidebar]\nagents_only = false\n[ui]\nstatus_indicators = \"dots\"\n",
        )));
        assert!(!config.ui.sidebar.agents_only);
        assert!(config.ui.sidebar.overlay);
        assert_ne!(config.ui.status_indicators, StatusIndicatorStyle::Symbols);
    }

    #[test]
    fn unparsable_user_config_is_returned_unchanged() {
        let broken = "[ui\nstatus_indicators = ";
        assert_eq!(layer(Some(broken)), broken);
    }

    #[test]
    fn user_arrays_replace_collie_arrays() {
        let merged = layer(Some("[keys]\nzoom = \"prefix+z\"\n"));
        let value: toml::Value = merged.parse().unwrap();
        assert_eq!(value["keys"]["zoom"].as_str(), Some("prefix+z"));
        assert_eq!(value["keys"]["copy_mode"].as_str(), Some("prefix+v"));
    }
}

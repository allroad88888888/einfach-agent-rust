//! `AGENT_REMOTE_TOOL_TIMEOUT_MS` 到运行时超时的严格配置边界。

use std::time::Duration;

const ENV_NAME: &str = "AGENT_REMOTE_TOOL_TIMEOUT_MS";

pub fn from_environment() -> Result<Option<Duration>, String> {
    match std::env::var(ENV_NAME) {
        Ok(value) => parse(Some(&value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{ENV_NAME} 必须是正整数毫秒")),
    }
}

fn parse(value: Option<&str>) -> Result<Option<Duration>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let millis = value
        .parse::<u64>()
        .ok()
        .filter(|millis| *millis > 0)
        .ok_or_else(|| format!("{ENV_NAME} 必须是正整数毫秒"))?;
    Ok(Some(Duration::from_millis(millis)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_value_preserves_runtime_default() {
        assert_eq!(parse(None).unwrap(), None);
    }

    #[test]
    fn positive_milliseconds_are_accepted() {
        assert_eq!(
            parse(Some("750")).unwrap(),
            Some(Duration::from_millis(750))
        );
    }

    #[test]
    fn invalid_or_zero_values_are_rejected() {
        assert!(parse(Some("0")).is_err());
        assert!(parse(Some("later")).is_err());
    }
}

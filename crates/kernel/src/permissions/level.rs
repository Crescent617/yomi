use serde::{Deserialize, Serialize};

/// 工具危险级别 / 自动批准阈值
///
/// 用于表示：
/// - 工具的固有危险级别（Tool Level）
/// - 用户配置的自动批准阈值（Auto-approve Threshold）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// 只读、无副作用的操作
    #[default]
    Safe,
    /// 可修改，但可撤销的操作（如编辑文件）
    Caution,
    /// 破坏性、不可撤销或影响外部的操作
    Dangerous,
}

impl std::str::FromStr for Level {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "safe" => Ok(Self::Safe),
            "caution" => Ok(Self::Caution),
            "dangerous" => Ok(Self::Dangerous),
            _ => Err(format!("unknown level: {s}")),
        }
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Safe => "safe",
            Self::Caution => "caution",
            Self::Dangerous => "dangerous",
        };
        write!(f, "{s}")
    }
}

/// 检查工具级别是否超过阈值
/// 返回 true 表示需要用户确认
pub const fn exceeds_threshold(tool_level: Level, threshold: Level) -> bool {
    let tool_value = tool_level as u8;
    let threshold_value = threshold as u8;
    tool_value > threshold_value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_level_ordering() {
        assert!(Level::Safe < Level::Caution);
        assert!(Level::Caution < Level::Dangerous);
    }

    #[test]
    fn test_exceeds_threshold() {
        // Safe threshold: only Safe passes
        assert!(!exceeds_threshold(Level::Safe, Level::Safe));
        assert!(exceeds_threshold(Level::Caution, Level::Safe));
        assert!(exceeds_threshold(Level::Dangerous, Level::Safe));

        // Caution threshold: Safe and Caution pass
        assert!(!exceeds_threshold(Level::Safe, Level::Caution));
        assert!(!exceeds_threshold(Level::Caution, Level::Caution));
        assert!(exceeds_threshold(Level::Dangerous, Level::Caution));

        // Dangerous threshold: all pass
        assert!(!exceeds_threshold(Level::Safe, Level::Dangerous));
        assert!(!exceeds_threshold(Level::Caution, Level::Dangerous));
        assert!(!exceeds_threshold(Level::Dangerous, Level::Dangerous));
    }

    #[test]
    fn test_from_str() {
        use std::str::FromStr;

        assert_eq!(Level::from_str("safe"), Ok(Level::Safe));
        assert_eq!(Level::from_str("CAUTION"), Ok(Level::Caution));
        assert_eq!(Level::from_str("Dangerous"), Ok(Level::Dangerous));
        assert!(Level::from_str("invalid").is_err());
    }
}

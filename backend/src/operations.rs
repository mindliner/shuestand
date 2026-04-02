use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

pub const OPERATION_MODE_KEY: &str = "operation_mode";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationMode {
    Normal,
    Drain,
    Halt,
}

impl OperationMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            OperationMode::Normal => "normal",
            OperationMode::Drain => "drain",
            OperationMode::Halt => "halt",
        }
    }
}

impl fmt::Display for OperationMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for OperationMode {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "normal" => Ok(OperationMode::Normal),
            "drain" => Ok(OperationMode::Drain),
            "halt" => Ok(OperationMode::Halt),
            _ => Err("invalid operation mode"),
        }
    }
}

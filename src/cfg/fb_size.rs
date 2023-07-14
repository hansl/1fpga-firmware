use merge::Merge;
use serde::Deserialize;

/// 0 - automatic, 1 - full size, 2 - 1/2 of resolution, 4 - 1/4 of resolution.
#[derive(Default, Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FramebufferSizeConfig {
    #[default]
    #[serde(alias = "0")]
    Automatic = 0,

    #[serde(alias = "1")]
    FullSize = 1,

    #[serde(alias = "2")]
    HalfSize = 2,

    #[serde(alias = "4")]
    QuarterSize = 4,
}

impl Merge for FramebufferSizeConfig {
    fn merge(&mut self, other: Self) {
        if other != FramebufferSizeConfig::default() {
            *self = other;
        }
    }
}

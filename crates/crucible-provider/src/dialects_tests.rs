//! Agreements between each Chat Completions dialect's halves, which the
//! shared wire cannot name a vendor to check.

use crate::completions::wire::Keeps;
use crate::completions::{Dialect, Reasoning};

/// A model whose reasoning goes back to its vendor is one whose dialect keeps
/// reasoning at all: the two are written apart, and a dialect that keeps
/// nothing would send back nothing, or empty strings, without a word.
#[test]
fn every_model_whose_reasoning_goes_back_is_of_a_dialect_that_keeps_it() {
    fn agrees<D: Dialect>(models: &[&str]) {
        for model in models {
            assert!(
                D::reasoning(model) == Reasoning::Unread || <D::Kept as Keeps>::KEEPS,
                "{model}"
            );
        }
    }

    agrees::<crate::deepseek::DeepSeekChat>(&["deepseek-flash", "deepseek-v4-pro"]);
    agrees::<crate::mimo::MimoChat>(&["mimo-v2.6-pro", "mimo-v2.6-flash"]);
    agrees::<crate::minimax::MiniMaxChat>(&["MiniMax-M3", "MiniMax-M2.7"]);
    agrees::<crate::moonshot::Kimi>(&[
        "k3",
        "k3-256k",
        "kimi-for-coding",
        "kimi-for-coding-highspeed",
    ]);
    agrees::<crate::qwen::QwenChat>(&[
        "qwen3.8-max",
        "qwen3.8-flash",
        "qwen3.7-plus",
        "qwen3.6-plus",
    ]);
    agrees::<crate::zai::ZaiChat>(&["glm-5.3", "glm-5.3-flash", "glm-5.2"]);
}

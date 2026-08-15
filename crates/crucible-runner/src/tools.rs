//! The tools a session offers, by name.

use crucible_core::{Tool, ToolSchema};

/// Every tool the model may call.
///
/// A list rather than a map. A session offers a handful of tools, and walking
/// six names costs less than hashing one; it also keeps the order they were
/// added, which is the order they are advertised in.
#[derive(Debug, Default)]
pub struct Tools {
    tools: Vec<Box<dyn Tool>>,
    schemas: Vec<ToolSchema>,
}

impl Tools {
    /// A session with nothing to call yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Offers one more tool.
    ///
    /// A name already present is replaced rather than shadowed. Two tools
    /// answering to one name is a wiring mistake, and the shadowed one would
    /// still be advertised to the model — which would then call something that
    /// never runs.
    pub fn add(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name();

        if let Some((present, schema)) = self
            .tools
            .iter_mut()
            .zip(&mut self.schemas)
            .find(|(present, _)| present.name() == name)
        {
            *schema = ToolSchema {
                name: tool.name(),
                schema: tool.schema(),
            };
            *present = tool;
        } else {
            self.schemas.push(ToolSchema {
                name: tool.name(),
                schema: tool.schema(),
            });
            self.tools.push(tool);
        }
    }

    /// The tool the model named, if there is one.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|tool| tool.name() == name)
            .map(|tool| &**tool)
    }

    /// Every tool, as a provider advertises them.
    #[must_use]
    pub fn schemas(&self) -> &[ToolSchema] {
        &self.schemas
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::ToolArgs;

    use super::*;
    use crate::fake::{Fixed, changing};

    #[test]
    fn a_tool_is_found_by_the_name_the_model_used() {
        let mut tools = Tools::new();
        tools.add(Box::new(Fixed::new("read")));
        tools.add(Box::new(Fixed::new("write")));

        assert_eq!(tools.find("write").map(Tool::name), Some("write"));
    }

    #[test]
    fn a_name_no_tool_answers_to_finds_nothing() {
        let mut tools = Tools::new();
        tools.add(Box::new(Fixed::new("read")));

        assert!(tools.find("frobnicate").is_none());
    }

    #[test]
    fn a_tool_added_twice_is_offered_once_and_the_later_one_answers() {
        // Both would otherwise be advertised, and the model calling the name
        // would reach whichever the search happened to meet first.
        let mut tools = Tools::new();
        tools.add(Box::new(Fixed::new("read")));
        tools.add(Box::new(Fixed::new("read").risking(changing())));

        assert_eq!(tools.schemas().len(), 1);
        assert_eq!(
            tools
                .find("read")
                .map(|tool| tool.sensitivity(&ToolArgs::new("{}"))),
            Some(changing())
        );
    }

    #[test]
    fn tools_are_advertised_in_the_order_they_were_added() {
        let mut tools = Tools::new();
        tools.add(Box::new(Fixed::new("read")));
        tools.add(Box::new(Fixed::new("grep")));

        let advertised: Vec<&str> = tools.schemas().iter().map(|tool| tool.name).collect();

        assert_eq!(advertised, ["read", "grep"]);
    }
}

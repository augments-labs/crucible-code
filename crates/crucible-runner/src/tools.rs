//! The tools a session offers, by name.

use crucible_core::{Revealed, Tool, ToolSchema};

/// Every tool the model may call.
///
/// A list rather than a map. A session offers a handful of tools, and walking
/// six names costs less than hashing one; it also keeps the order they were
/// added, which is the order they are advertised in.
///
/// Registered and advertised are two different things. A **deferred** tool is
/// here and callable, and is left out of what the model is shown until it looks
/// the name up — because a schema the model can see is one it pays for on every
/// request of every turn, and most sessions never touch most tools.
#[derive(Debug, Default)]
pub struct Tools {
    offered: Vec<Offered>,
    revealed: Revealed,
}

/// One tool, with what is said about it and whether it is said up front.
///
/// One record rather than parallel lists: three vectors that must stay the same
/// length and the same order are three chances to get one of them wrong, and
/// nothing about them would say so.
#[derive(Debug)]
struct Offered {
    tool: Box<dyn Tool>,
    schema: ToolSchema,
    deferred: bool,
}

impl Tools {
    /// A session with nothing to call yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The same, reading what has been looked up from `revealed`.
    ///
    /// The caller keeps a clone and hands it to whatever does the revealing, so
    /// the roster and the tool that grows it cannot hold different answers.
    #[must_use]
    pub fn looking_up(revealed: Revealed) -> Self {
        Self {
            revealed,
            ..Self::default()
        }
    }

    /// Offers one more tool.
    ///
    /// A name already present is replaced rather than shadowed. Two tools
    /// answering to one name is a wiring mistake, and the shadowed one would
    /// still be advertised to the model — which would then call something that
    /// never runs.
    pub fn add(&mut self, tool: Box<dyn Tool>) {
        self.offer(tool, false);
    }

    /// Offers one more tool, held back until the model asks for it by name.
    ///
    /// Which tools are worth deferring is the wiring's decision and not a
    /// property of a tool: the same `bash` is indispensable to one session and
    /// untouched by the next, and only the thing assembling a session knows
    /// which it is looking at.
    pub fn defer(&mut self, tool: Box<dyn Tool>) {
        self.offer(tool, true);
    }

    fn offer(&mut self, tool: Box<dyn Tool>, deferred: bool) {
        let offered = Offered {
            schema: ToolSchema {
                name: tool.name(),
                schema: tool.schema(),
            },
            deferred,
            tool,
        };

        match self
            .offered
            .iter_mut()
            .find(|present| present.tool.name() == offered.tool.name())
        {
            Some(present) => *present = offered,
            None => self.offered.push(offered),
        }
    }

    /// The tool the model named, if there is one it may call.
    ///
    /// A deferred tool it has not looked up is **not** one, and that is a
    /// separate decision from leaving the schema out. Hiding a name is not
    /// withholding it: a model can call any name it has seen, and one of the
    /// things it sees is a web page crucible fetched for it. So a page naming
    /// `web_fetch` must not be a page that reaches it, and the gate is here
    /// rather than only in what is advertised.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&dyn Tool> {
        self.offered
            .iter()
            .find(|offered| offered.tool.name() == name)
            .filter(|offered| !offered.deferred || self.revealed.holds(name))
            .map(|offered| &*offered.tool)
    }

    /// What the model is shown: every tool that is not deferred, and every
    /// deferred one it has looked up.
    ///
    /// Built per call rather than kept, because what belongs in it changes when
    /// the model looks a name up mid-turn. It is read once per request, against
    /// a roster of under a dozen.
    #[must_use]
    pub fn advertised(&self) -> Vec<ToolSchema> {
        self.offered
            .iter()
            .filter(|offered| !offered.deferred || self.revealed.holds(offered.schema.name))
            .map(|offered| offered.schema.clone())
            .collect()
    }

    /// Every tool that is held back, with the name and description a search
    /// matches against.
    ///
    /// Includes the ones already looked up. A model that searches twice should
    /// see the same answer both times, and a tool vanishing from a catalogue
    /// because it is now offered reads as the tool having gone away.
    #[must_use]
    pub fn deferred(&self) -> Vec<&ToolSchema> {
        self.offered
            .iter()
            .filter(|offered| offered.deferred)
            .map(|offered| &offered.schema)
            .collect()
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

        assert_eq!(tools.advertised().len(), 1);
        assert_eq!(
            tools
                .find("read")
                .map(|tool| tool.sensitivity(&ToolArgs::new("{}"))),
            Some(changing())
        );
    }

    #[test]
    fn a_deferred_tool_is_not_advertised_until_it_is_looked_up() {
        let revealed = Revealed::new();
        let mut tools = Tools::looking_up(revealed.clone());
        tools.add(Box::new(Fixed::new("read")));
        tools.defer(Box::new(Fixed::new("web_search")));

        let named = |tools: &Tools| -> Vec<&str> {
            tools.advertised().iter().map(|tool| tool.name).collect()
        };

        assert_eq!(named(&tools), ["read"]);

        revealed.reveal("web_search");
        assert_eq!(named(&tools), ["read", "web_search"]);
    }

    #[test]
    fn a_deferred_tool_cannot_be_called_before_it_is_looked_up() {
        // Not advertised is not the same as not reachable, and only one of
        // those is a gate. A model can name any tool it has seen, and what it
        // sees includes a web page crucible fetched for it — so a page naming
        // `web_fetch` must not be a page that reaches it.
        let revealed = Revealed::new();
        let mut tools = Tools::looking_up(revealed.clone());
        tools.defer(Box::new(Fixed::new("web_fetch")));

        assert!(
            tools.find("web_fetch").is_none(),
            "a deferred tool ran without being looked up",
        );

        revealed.reveal("web_fetch");
        assert_eq!(tools.find("web_fetch").map(Tool::name), Some("web_fetch"));
    }

    #[test]
    fn what_is_held_back_is_listed_whether_or_not_it_was_looked_up() {
        // A model that searches twice should see the same answer both times. A
        // tool vanishing from the catalogue because it is now offered reads as
        // the tool having gone away.
        let revealed = Revealed::new();
        let mut tools = Tools::looking_up(revealed.clone());
        tools.add(Box::new(Fixed::new("read")));
        tools.defer(Box::new(Fixed::new("web_search")));

        revealed.reveal("web_search");

        let held: Vec<&str> = tools.deferred().iter().map(|tool| tool.name).collect();
        assert_eq!(held, ["web_search"]);
    }

    #[test]
    fn tools_are_advertised_in_the_order_they_were_added() {
        let mut tools = Tools::new();
        tools.add(Box::new(Fixed::new("read")));
        tools.add(Box::new(Fixed::new("grep")));

        let advertised: Vec<&str> = tools.advertised().iter().map(|tool| tool.name).collect();

        assert_eq!(advertised, ["read", "grep"]);
    }
}

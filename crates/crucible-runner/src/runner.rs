//! The turn loop.
//!
//! Ask the model, run what it asked for, tell it what happened, ask again —
//! until it yields, the user stops it, or something goes wrong.
//!
//! Progress leaves through events, because the thread that draws is not this
//! one. The outcome leaves through the return value, because the caller is
//! what decides whether the session continues.

use std::sync::mpsc::Sender;

use crucible_core::{
    Ask, Cancel, Delta, Event, Message, Permission, Provider, Request, StopReason, ToolCall,
    Transcript, TurnError, TurnId,
};

use crate::answer::Answer;
use crate::post;
use crate::tools::Tools;
use crate::work::{Went, Work};

/// Which model to ask, and how.
#[derive(Debug, Clone)]
pub struct Model {
    /// The model's name, as the provider spells it.
    pub name: Box<str>,
    /// Ceiling on one response.
    pub max_tokens: u32,
    /// The system prompt, if the session has one.
    pub system: Option<Box<str>>,
}

/// Drives turns to completion.
///
/// Holds what outlives a turn: the provider, the tools, the session's
/// permission memory, and the transcript itself.
#[derive(Debug)]
pub struct Runner {
    provider: Box<dyn Provider>,
    tools: Tools,
    model: Model,
    permission: Permission,
    transcript: Transcript,
    turn: TurnId,
}

impl Runner {
    /// A session that has not said anything yet.
    #[must_use]
    pub fn new(provider: Box<dyn Provider>, tools: Tools, model: Model) -> Self {
        Self {
            provider,
            tools,
            model,
            permission: Permission::new(),
            transcript: Transcript::new(),
            turn: TurnId::FIRST,
        }
    }

    /// The conversation so far.
    #[must_use]
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// Takes one turn: the prompt, and the exchange until the model yields.
    ///
    /// # Errors
    ///
    /// [`TurnError`] when the provider failed or the user refused a tool. A
    /// tool that ran and did not like what it found is not an error — that
    /// goes back to the model as a result it can work around.
    pub fn turn(
        &mut self,
        prompt: &str,
        ask: &mut dyn Ask,
        events: &Sender<Event>,
        cancel: &Cancel,
    ) -> Result<StopReason, TurnError> {
        // Whatever stopped the last turn is spent. Clearing it here, on the one
        // thread that owns the loop, means no worker starts against a stale
        // request.
        cancel.reset();

        if !self.transcript.is_empty() {
            self.turn = self.turn.next();
        }
        post(events, Event::TurnStarted { turn: self.turn });
        self.transcript.push(Message::User(prompt.into()));

        loop {
            let answer = self.listen(events, cancel)?;
            let said = answer.stop();
            let (text, calls) = answer.finish();

            if let Some(stop) = Self::over(said, &calls) {
                // Calls the model did not finish asking for go no further. A
                // call is written to the transcript only once it has a result,
                // and these will never get one.
                self.transcript.push(Message::Agent {
                    text,
                    calls: Vec::new(),
                });
                return Ok(stop);
            }

            for call in &calls {
                post(events, Event::ToolRequested { call: call.clone() });
            }

            let (results, went) = Work {
                tools: &self.tools,
                permission: &mut self.permission,
                ask,
                events,
                cancel,
            }
            .round(&calls);

            self.transcript.push(Message::Agent { text, calls });
            self.transcript.push(Message::ToolResults(results));

            match went {
                Went::On => {}
                Went::Stopped(stop) => return Ok(stop),
                Went::Refused(name) => return Err(TurnError::Refused(name)),
            }
        }
    }

    /// Whether the turn is over, and why — or `None` to run the calls.
    fn over(said: Option<StopReason>, calls: &[ToolCall]) -> Option<StopReason> {
        match said {
            Some(StopReason::WantsTools) if !calls.is_empty() => None,
            // Waiting on nothing is yielding, whatever the provider called it.
            // Believing it instead would re-send an unchanged transcript and
            // ask the same question until the user noticed.
            Some(StopReason::WantsTools) | None => Some(StopReason::Yielded),
            Some(stop) => Some(stop),
        }
    }

    /// One request, read to the end.
    fn listen(&self, events: &Sender<Event>, cancel: &Cancel) -> Result<Answer, TurnError> {
        let mut stream = self.provider.stream(self.request(), cancel)?;
        let mut answer = Answer::new(self.provider.name());

        while let Some(delta) = stream.next() {
            match delta? {
                Delta::Text(text) => {
                    answer.say(&text);
                    post(events, Event::Delta { text });
                }
                Delta::ToolStarted { id, name } => answer.calling(id, name),
                Delta::ToolArgs(fragment) => answer.arguments(&fragment)?,
                Delta::Stopped(stop) => answer.stopped(stop),
            }
        }

        Ok(answer)
    }

    /// What to send this round.
    fn request(&self) -> Request {
        Request {
            model: self.model.name.clone(),
            // Cloned because a `Request` owns what it sends: the provider may
            // hold it for as long as the socket is open, and the loop keeps
            // appending to the transcript meanwhile. It is the same order of
            // work as serialising it, which the provider does on the next line.
            transcript: self.transcript.clone(),
            tools: self.tools.schemas(),
            max_tokens: self.model.max_tokens,
            system: self.model.system.clone(),
        }
    }
}

#[cfg(test)]
mod tests;

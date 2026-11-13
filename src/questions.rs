use super::*;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    pub question: String,
    pub blocking: bool,
    pub created_at_ms: u64,
    pub wait_until_ms: Option<u64>,
    pub wait_outcome: Option<String>,
    pub call_ids: Vec<String>,
    pub answer: Option<Answer>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub text: String,
    pub message_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptArgs {
    pub question: String,
    pub blocking: bool,
    pub wait_seconds: Option<u64>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl Question {
    pub fn receipt(&self) -> Value {
        let status = if self.answer.is_some() {
            "answered"
        } else if let Some(outcome) = &self.wait_outcome {
            outcome
        } else if self.blocking {
            "waiting"
        } else {
            "open"
        };
        json!({"question_id":self.id,"question":self.question,"status":status,
        "blocking":self.blocking,"wait_until_ms":self.wait_until_ms,
        "answer_message_id":self.answer.as_ref().map(|a| &a.message_id),
        "message": if self.answer.is_some() {
                "The answer was submitted as a user message. Do not ask this question again."
        } else {
            "The question is saved and remains answerable. Do not repeat it. Silence is not an answer or approval."
        }})
    }
}

impl Record {
    pub(super) fn cancel_queued_question_answers(&mut self) {
        for question in &mut self.view.questions {
            if question.answer.as_ref().is_some_and(|answer| {
                self.inputs
                    .iter()
                    .any(|input| input.id == answer.message_id && input.order <= self.stop_through)
            }) {
                question.answer = None;
                question.wait_outcome = Some("answer_cancelled".into());
            }
        }
    }
}

impl Hub {
    pub fn recover_questions(&self, id: &str, session: &mut Session) -> Result<()> {
        self.update(id, |r| {
            let mut recovery = session.recover_questions(&mut r.view.questions);
            recovery.extend(session.recover());
            for (kind, text) in recovery {
                r.view.entries.push(Self::make_entry(&kind, text));
            }
            r.engine = session.clone();
            Ok(())
        })
    }

    pub fn register_question(&self, id: &str, call_id: &str, args: PromptArgs) -> Result<Question> {
        let text = args.question.trim();
        if text.is_empty() || text.chars().count() > 8000 {
            bail!("question must contain 1 to 8000 characters.");
        }
        if args.wait_seconds.is_some_and(|s| !(1..=86400).contains(&s)) {
            bail!("wait_seconds must be 1 to 86400, or null.");
        }
        if !args.blocking && args.wait_seconds.is_some() {
            bail!("Use wait_seconds=null for a nonblocking question.");
        }
        self.update(id, |r| {
            if r.view.archived || r.view.status == "ending" {
                bail!("Session has ended.");
            }
            if let Some(question) = r.view.questions.iter_mut().find(|q| {
                q.call_ids.iter().any(|call| call == call_id)
                    || (q.answer.is_none() && q.question == text)
            }) {
                if !question.call_ids.iter().any(|call| call == call_id) {
                    question.call_ids.push(call_id.to_owned());
                }
                return Ok(question.clone());
            }
            if r.view
                .questions
                .iter()
                .filter(|q| q.answer.is_none())
                .count()
                >= 32
            {
                bail!(
                    "There are already 32 unanswered questions. Continue without posting another."
                );
            }
            let created_at_ms = now_ms();
            let question = Question {
                id: Uuid::new_v4().to_string(),
                question: text.into(),
                blocking: args.blocking,
                created_at_ms,
                wait_until_ms: args
                    .blocking
                    .then(|| created_at_ms + args.wait_seconds.unwrap_or(300) * 1000),
                wait_outcome: None,
                call_ids: vec![call_id.into()],
                answer: None,
            };
            r.view
                .entries
                .push(Self::make_entry("assistant", text.into()));
            r.view.questions.push(question.clone());
            Ok(question)
        })
    }

    pub async fn prompt_user(&self, id: &str, call_id: &str, args: PromptArgs) -> Result<Value> {
        let question = self.register_question(id, call_id, args)?;
        if !question.blocking || question.answer.is_some() || question.wait_outcome.is_some() {
            return Ok(question.receipt());
        }
        // The worker is inside this tool, so it is the only consumer of its wake signal.
        let wake = crate::blocking::run(|| self.0.data.lock().unwrap().sessions[id].wake.clone());
        loop {
            let (current, has_input) = crate::blocking::run(|| {
                let data = self.0.data.lock().unwrap();
                let record = &data.sessions[id].record;
                (
                    record
                        .view
                        .questions
                        .iter()
                        .find(|q| q.id == question.id)
                        .unwrap()
                        .clone(),
                    record.inputs.iter().any(|i| i.order > record.stop_through),
                )
            });
            if current.answer.is_some() || current.wait_outcome.is_some() {
                return Ok(current.receipt());
            }
            let remaining = current.wait_until_ms.unwrap().saturating_sub(now_ms());
            if remaining == 0 || has_input {
                return self.update(id, |r| {
                    let q = r
                        .view
                        .questions
                        .iter_mut()
                        .find(|q| q.id == question.id)
                        .unwrap();
                    // An answer may have committed between the read and this update.
                    if q.answer.is_none() {
                        q.wait_outcome = Some(
                            if has_input {
                                "continued_for_user_message"
                            } else {
                                "timed_out"
                            }
                            .into(),
                        );
                    }
                    Ok(q.receipt())
                });
            }
            tokio::select! {
                _ = wake.notified() => (),
                _ = tokio::time::sleep(Duration::from_millis(remaining)) => (),
            }
        }
    }

    pub fn answer_question(&self, id: &str, question_id: &str, text: String) -> Result<String> {
        if text.trim().is_empty() || text.chars().count() > 32000 {
            bail!("Answer must contain 1 to 32000 characters.");
        }
        crate::blocking::run(|| {
            let mut permit = self.reserve_persistence(id, 1)?;
            let mut data = self.0.data.lock().unwrap();
            if data.closing {
                bail!("Server is shutting down.");
            }
            let (message_id, receipt) = self.update_locked(&mut data, id, Some(&mut permit), |r| {
                if r.view.archived || r.view.status == "ending" {
                    bail!("Session has ended. Create a new agent instead.");
                }
                let q = r.view.questions.iter_mut().find(|q| q.id == question_id)
                    .context("Unknown question.")?;
                if let Some(answer) = &q.answer {
                    if answer.text == text { return Ok(answer.message_id.clone()); }
                    bail!("This question has already been answered. Send a follow-up message to change the answer.");
                }
                if r.inputs.len() >= 32 { bail!("Session input queue is full."); }
                let message_id = Uuid::new_v4().to_string();
                let content = format!("Reply to: {}\n\n{}", q.question, text);
                q.answer = Some(Answer { text, message_id: message_id.clone() });
                q.wait_outcome = Some("answered".into());
                r.order += 1;
                r.inputs.push_back(Input { id: message_id.clone(), text: content.clone(),
                    order: r.order, compact: false, clear: false });
                r.view.entries.push(Entry { id: message_id.clone(), kind: "user".into(), text: content.into(),
                    replaces: None, input_batch: None });
                r.view.pending.push(message_id.clone());
                r.paused = false;
                r.listen_on_restart = Some(true);
                Ok(message_id)
            })?;
            drop(data);
            receipt.wait()?;
            if let Some(handle) = self
                .0
                .data
                .lock()
                .unwrap()
                .sessions
                .get(id)
                .filter(|h| !h.record.view.archived)
            {
                handle.wake.notify_one();
            }
            Ok(message_id)
        })
    }
}

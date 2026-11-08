# Audience understanding before interface changes

> [!WARNING]
> **STRONG MODEL FAILURE CASE — HIGH PROPENSITY**
> Andy explicitly assigned this classification to the trace on 2026-11-08.
> The repeated failure: turning a demonstrated judgment into a narrow rule
> about the current example, then proposing implementation before understanding
> the principle. This recurred despite explicit feedback and relevant guidance.
> Preserve the correction sequence, not only the final successful paraphrase.

2026-11-08, v4 preview to v5 planning. This case preserves feedback and its
sequence. It is developer context, not a runtime instruction or a settled UI
design. The interpretations below are Woo's; the quoted user words are Andy's.

## Two judgments

Andy clarified after requesting the failure classification:

> well technically you have two judgements now

Keep these distinct:

1. **Customer obsession:** use the accumulated understanding of the actual
   customer to choose the approach that best serves them. Rephrasing the ask,
   describing a generic user, or asking questions already answered is not that
   work. Ask only about gaps whose answers would change the choice; each
   question must justify the attention it costs.
2. **Minimalism:** question what needs to exist to satisfy the actual purpose.
   Choose the simplest sufficient solution. This is not a visual style, a fixed
   feature count, or a prohibition on additions.

The shared failure was replacing each judgment with a narrow rule drawn from
its immediate example. The common mechanism does not make the two judgments
interchangeable. The full exchanges and qualifications follow.

## Trajectory

Andy had asked for live agent activity visible in his periphery, close reading
of one session without losing the others, full access to the available stream,
and a real terminal. He preferred web for future swarm communication. His
VS Code screenshot illustrated an arrangement he liked; he had expressly
warned against assuming that it specified the new interface.

V4 delivered an automatic grid with equal-sized panes, a raw labeled event
stream, repeated input controls, and a modal for creating sessions. Functional
browser checks passed. Andy rejected its UX and questioned the judgment behind
the code. Woo explained several implementation choices and their costs.

## Feedback and failed response

Andy:

> its good that you reflected; i wont address point by point because there are so many issues; i need you to look at these words and hopefully the judgement can be conferred; 1. a good ui/ux is intuitive; 2. a good ui/ux has every label earn its place, if it does not need to be there, it must not be there; in this time one needs to understand their audience, 3. if you put no effort in understanding your audience it shows in the ui/ux

Woo recorded the words and described Andy as an experienced operator. Andy
then asked:

> so?

Woo's proposed action:

> I’ll make a first v5 pass around reading and directing agents: readable Markdown, tool activity without JSON wrappers or duplicated arguments, compact inputs, and fewer repeated labels. Full event details will remain accessible.
>
> I’ll use your existing conversation to judge the result before adding more controls.

The turn inspected code but made no UI edits. Andy interrupted:

> haha you havent even taken a moment to understand your audience, this shows you have truly not taken proper effort to understand my advice; re-read the kb

Woo read the KB profile and calibration cases, then summarized Andy's frequent
context switches, limited attention, desire for autonomous work, and use of
the live trace to investigate agent behavior. Woo acknowledged that the UI
changes had been chosen before establishing the activity they should serve.

Andy clarified:

> customer obsession isnt just rephrasing your question, it is adopting the paradigm that most satisfies your customer; here, i assume you did enough customer research about me, to understand what i want... to be fair you just needed to properly listen to my asks as i have been asking them to you; you may use this time to do any discovery you havent done already. but after that you need to find a way to get to the core of the problem without annoying the user, this means if you can ask one question to solve this problem, it will be the only question you ask. this means not reaching for the best number of problems you feel is best, this means its not 10 questions because 10 is a nice number, its 1-4-6-12 if the questions justify their existence. im tired of having to clarify over and over, therefore i suggest you prepare your questions and refine over them for as many passes you feel necessary

## Preservation and learning

Andy:

> reminder this is my taste, im doing you a favor in giving you the deepest look of my taste; everythingi write is my taste, and something you could learn from, what will you do with that?

Woo answered:

> I’ll use it to learn **how you make decisions**, including what you reject, what you accept, and why you interrupt. The whole sequence matters.
>
> - Preserve your words and their context in the existing Boid/Marigold records, with my interpretations clearly separate.
> - Revise the assumption behind a mistake. Here, I kept assuming visible implementation was the right response, even while you were asking me to develop judgment.
> - Apply that understanding to new decisions without making you repeat the lesson. Your question-count example means each question must justify its cost, not “always ask one question.”
> - Judge whether I learned by subsequent choices and how often you have to correct the same pattern. Recording your advice is not evidence of learning.
>
> I also need to retain the tensions in your taste: speed with thought, autonomy with understanding, restraint with sufficient capability. Turning either side into an absolute rule produces the swings you’ve been correcting.

Andy:

> great, preservation, however you feel is best, is the right move; keep going, back to task;

## Interpretation and unresolved work

Woo's interpretation: the repeated error was selecting interface changes before
using the accumulated evidence to choose the interaction paradigm. Quoting a
principle, naming a user category, and listing plausible improvements did not
perform that work. The correction does not reject Markdown rendering or compact
inputs on their merits; it rejects treating them as the answer without the
missing reasoning.

Use prior answers to eliminate questions. Ask about an unresolved distinction
only when its answer would change the design. The number follows that need;
there is no preferred count. Do not turn this into a mandatory questionnaire
or make Andy supply a component specification.

V5 UX discovery remains active. No redesign or claim of improved judgment has
been validated. Daybreak is separately backlogged; identity work is deferred.

## Discovery: the screenshot did not endorse Codex's session UX

Woo asked:

> Is interacting with a Codex terminal session already close to what you want there, or does that interaction itself need a fundamental change?

Andy answered:

> good, you're closest to the problem so far, this is the most pointed question you've asked so far; i dont like codex session ux, i hate it a lot actually; you've overindexed on the screenshot ive sent and never asked me what i really want

This answers the earlier baseline question: Codex's session interaction is
not the desired baseline. The screenshot and surrounding statements support
peripheral awareness of several agents, not approval of each session's UX.
Andy praised the question's relevance, not a proposed design. The desired
interaction with an individual agent remains to be elicited; do not substitute
another familiar interface or infer it from this rejection alone.

Andy then supplied a positive reference:

> claude code is probably the closest so far; why?

This establishes Claude Code as the closest reference so far, not blanket
approval of its features or a decision to clone it. The reasons remain to be
confirmed; Woo's analysis of hierarchy, direct interaction and control of
attention is an interpretation, not an additional user requirement.

Andy then gave the concrete comparison:

> good hypotheses, i wont acknowledge that they are correct; instead ill walk through what i hate about codex and what i liked more back when iw as using claude code; I hate how slow scrolling is in codex, how ugly codex is, and how busy codex feels; its unreadable in the small panes we seek to represent agents; not even speaking of the interaction dynamics; claude code is simpler, thats it.

The supported priorities are scrolling responsiveness, visual simplicity,
appearance, and readability in small agent panes. Andy explicitly withheld
confirmation of Woo's earlier hypotheses. Do not treat message queueing,
interruption, keyboard conventions, or collapsed detail as accepted preferences
from this exchange. Interaction dynamics remain a separate discussion. Judge
the visual result at the intended small pane size, not only in a large view.

## Recurrence: interpreting minimalism as a rule about interactions

Andy asked for Codex's interaction count and state machine, then asked how much
Boid actually needed. Woo proposed two agent-specific actions, send and stop,
three execution states, and independent following/browsing behavior. Asked for
a pure count, Woo answered:

> Codex: **~60**
> Boid, proposed: **2**

Evidence qualification: the underlying source count was 65 slash-command
variants in Codex reference checkout a90c808; Boid's two counted only the
proposed agent-specific controls. Ordinary editing and browsing were excluded
from Boid's count. This comparison illustrates a design discussion, not a
normalized measurement of total product complexity.

Andy:

> great! lets make sure we explicitly plan from this perspective; minimalism doesnt just mean we use a dark background or we use less font adding subtitles. it means we do what we just did

Woo responded:

> I’ll make the two-action model the starting point for v5: send and stop. Every additional interaction or mode will need a concrete reason to exist.

Andy interrupted before any edit:

> no you dont deserve to reach implementation yet, we're still correcting your behavior; and hoping you archive my taste so its ready for boid

Woo then said:

> Implementation stays paused. I’ll archive this exchange as a correction to my judgment: reducing the interactions and modes a user must manage comes before styling, and your agreement with that principle was not permission to start building.

Andy:

> is it about the interactions? is it about the modes?

Woo's answer:

> Interactions and modes were the example. **The principle is questioning what needs to exist at all to satisfy your actual purpose.**
>
> I took a judgment you demonstrated and immediately narrowed it into another rule about UI components. The same judgment applies to features, code, documents, labels, and even the questions I ask you.
>
> The goal isn’t the lowest count. It’s the simplest sufficient solution, with every addition earning its cost. Sometimes that requires adding something.

Andy accepted that interpretation and requested this classification:

> yes, good, thats it; make sure this is also marked visibly as a trace thats an example of a strong model failure case (high propensity)

## Failure mechanism and present outcome

The observations show a recurring failure in this trajectory: audience feedback
became a cosmetic implementation plan; discovery advice became a restatement
of the question; minimalism became a rule about interaction counts. The agent
could repeat the words while continuing to select the next step from the same
unexamined assumptions. Relevant guidance had been read, so this case must not
be reduced to a missing-instruction incident.

Andy's high-propensity classification is explicit. A population frequency or
immutable model trait has not been measured. His acceptance establishes that
the final explanation captured the intended principle; it does not establish
that the agent will apply it reliably. Implementation remains paused. Retain
this full case for Boid's later learning and developer calibration without
automatically adding the whole failure trace to ordinary runtime prompts.

## First implementation authorized

Andy approved most of the proposed first pane pass, with this specific steering:

> good choice, i approve of most of your position, i havent read all of it; one thing to clarify: I hate buttons. enter should send; shift+enter should multiline; I liked codex's system where if you send  a message to agent as its working, that gets queued until next good discrete instance aswell as a send immediately if you esc, this is good behavior, lets follow this pattern if the user is "selected" on a pane. one more note, i dont know if this is from you or otherwise, but when i click on a div, it outlines it with this annoying blue outline? i can explain later if you dont know

He added:

> nit: remove rubberbanding when trying to scroll down past bottom of page

And requested deployment:

> update server, and ping me when done

This supersedes the implementation pause for the first pane revision. It does
not approve every earlier proposal or withdraw the rejection of Codex's broader
session UX. The named queue/interrupt behavior is a specific positive reference.

# A correct connection label that did not belong in the composer

2026-11-10, Boid 1.1.7 through 1.1.9. This case preserves Andy's correction,
Cataclysm's choices, and the limits of the resulting lesson. It is development
context; read the [bootstrap caution](../README.md) before using cases.

## What happened

Andy requested `/login` with API-key or subscription access, without an account
page. The first implementation opened inside an agent pane but changed only the
default for future agents. Andy then corrected the scope: the selected connection
must affect that pane's agent. He also requested removal of these strings:

> Manage usage
> New agents: ChatGPT subscription · gpt-6-sol
> Selection applies to new agents. Existing agents keep their connection.

He separately directed: "model selection should happen under /model not /login"
and asked for a TODO to redesign the unsatisfactory login flow and screen.

Cataclysm shipped the functional repair in 1.1.8, commit `41ebb4a`, merged as
`7af4da3`. The repair removed those strings but retained a shorter, persistent
"Using ChatGPT plan" label inside every subscription agent's composer. Browser
checks explicitly expected the label to appear after selecting subscription.

All seven production agents subsequently completed fresh requests recorded as
subscription use. That verified the connection and request route. It did not
establish that the added composer label was useful or wanted.

## Andy's correction

> remove that text that says using chatgpt plan; dude you really gotta stop adding shit i didnt ask for; remove it from the message box; (im ok with you adding shit i dindt ask for but when you fuck up like this it shows a difference in taste, i dont really have a good re-calibration point. maybe add as a case study in the boid kb in mm-west-2

Source: Andy's direct message in the Cataclysm conversation on 2026-11-10.
The earlier removal and model-selection quotations come from the same exchange.
At the time of correction, `src/web/app.js` created `ui.billingLabel` and toggled
it in `updateProfile`; `src/web/style.css` styled `.billing-route`.

## Interpretation and failed calibration

Cataclysm treated accurate connection information as sufficient reason to keep
it visible in the message box. After the first copy correction, shortening the
label preserved the underlying judgment that had not been established: that
this persistent status deserved space on that surface. Tests reinforced the
implementation choice instead of assessing whether it served the interaction.

The related [audience and interface case](audience-before-interface.md) was
already available and had been read. This recurrence is evidence that reading
and repeating the minimalism principle did not reliably change the choice.

Andy's qualification matters: unsolicited additions are allowed. The lesson is
not to require permission for every detail, remove every status indicator, or
ban useful additions. The practical calibration hypothesis is to judge an
addition by the decision it helps Andy make at that location and time, using
prior corrections as context. Persistent text in a frequently used composer
needs a stronger reason than being technically true. A label may be appropriate
elsewhere or for another interaction; that was not established here.

## Repair and remaining uncertainty

The targeted repair deletes the composer label and its CSS, without a substitute
badge, tooltip, or message. Connection selection, model selection, and billing
records retain their functions. The existing browser check now verifies that
switching to subscription leaves no such element in the composer.

The larger login-flow and screen redesign remains a v2 TODO. Removing this label
can be verified in the served UI; the hypothesis about better taste requires
later decisions and feedback. A passing check, a recorded case, or a deployment
does not establish that the calibration problem is solved.

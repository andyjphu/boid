# Boid case bank

These cases preserve Andy's words, the agent's choices, and the resulting
feedback. They are available to Boid when it develops itself, as well as to
other assigned developers. Read the bootstrap caution before using failure
cases: expected failures must never become behavioral goals.

| Case | When it helps | Evidence and limits |
| --- | --- | --- |
| [Reviewability without disposable interfaces](intermediate-interfaces.md) | Choosing implementation stages, temporary interfaces, or review boundaries | Andy praised reconsidering the mechanism while preserving its useful purposes. The reasoning was accepted; an implementation outcome was not observed in that exchange. |
| [Audience understanding before interface changes](audience-before-interface.md) | Understanding dense feedback, questioning what needs to exist, preserving a reference interaction, explaining a disputed choice | Repeated failures despite feedback; also a positive example of explaining a defensible choice. Andy's high-propensity classification is preserved as his assessment, not a measured population rate. |
| [Preserving the whole interaction through corrections](whole-interaction.md) | Revising an interaction after feedback, preserving a working environment, choosing representative verification | Real defaults led to a partial custom repair; a new preview lost terminal styling; dynamic cwd exposed blocking persistence. Includes source-pinned exchanges, measured repair evidence, and Andy’s explicitly approved v0.7.2 prompt-clearance result. Long-term guidance efficacy remains unknown. |

## Reading and curating cases

Begin with the request and accepted baseline. Read the correction sequence,
including praise, ambiguity, and later changes of direction. A short user turn
can carry several independent meanings. Keep the original wording available
when drawing those meanings out; an agent summary may omit a distinction that
a later reviewer needs. An interpretation should identify its supporting
exchange and remain open to correction.

Cases supply evidence for judgment. An analogous task still needs its own
reasoning. A local failure does not establish a universal ban, and accepting
an explanation does not show that subsequent behavior improved. Keep positive
examples and contrary evidence alongside failures so review does not become a
search for more restrictions.

For a new case, preserve enough of the original request, agent response,
feedback, and later outcome to reconstruct the decision. Include trace
references and verbatim excerpts when local trace files will not travel with
the repository. State which evidence was directly inspected, reported by
another worker, or inferred. Retain earlier repairs and their outcomes when
later feedback changes the analysis. Source excerpts should contain the
relevant visible exchange; exclude credentials and unrelated private material.

Maintain the case itself as later evidence arrives, preserving dated earlier
interpretations. Case selection is curated and may overrepresent difficult
episodes. More cases give reviewers useful comparisons; their count alone
does not establish learning or a failure rate.

The [meta-review proposal](../meta-review.md) describes possible generic and
meta-review Boid templates. Selected cases can inform a current task or a
guidance revision. Loading the entire bank into every ordinary session is not
the proposed default.

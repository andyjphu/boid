# Agent tabs

Working labels shimmer even when another subpane is selected. Reduced-motion
preferences keep the label readable without animation.

`set_agent_color({"to":null,"color":"cyan"})` colors the caller's tab.
`to` can instead name a peer by session ID or an unambiguous name. Captains choose
a non-grey team color and apply it to assigned agents. Colors are identity
metadata, survive clears, compaction, moves and restarts, and appear in status
context and `list_agents`. Recoloring does not wake or interrupt an agent.

The palette is grey (default), red, orange, yellow, lime, green, mint, cyan, sky,
blue, violet, magenta and rose. Fast mode displays its existing red background;
the chosen team color returns when fast mode ends.

Drag a tab onto a numbered screen button to move it. The destination highlights
blue. The moved tab becomes a pane appended after existing destination panes.
Execution and credentials continue; the source screen remains selected.

Tab focuses the next visible agent composer in screen reading order. Shift+Tab
moves backward. Navigation wraps, skips terminals and inactive subpanes, and
preserves unsent drafts.

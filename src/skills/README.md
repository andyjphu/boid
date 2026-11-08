# Bundled skills

New sessions receive a short skill catalog with their instructions. The model
uses `read_skill` to load the complete text when it applies. Humanizer is the
first bundled skill and applies to prose work from the start of v0.8.

The session saves the skill text and version alongside its original instructions
and tool definitions. Restarting or reopening a session keeps that captured
environment. Sessions created before skill support retain their old tools and
instructions; create a new session to use the bundled skills. Changing the
bundle affects new sessions. Loaded skill text appears in the ordinary tool
trace and model conversation. Loading does not prove that the model applied it.

Humanizer is copied without modification from Andy's maintained v3.1.0 at
`project-marigold/guidance/skills/humanizer/SKILL.md`. Its upstream is
https://github.com/blader/humanizer; the bundled MIT license credits Siqi Chen.
Preserve that license and the source/version when updating the copy.
The bundled `SKILL.md` SHA-256 is
`0612f1dfb1672b0ea9b97e139bf1f06cabe98d8b27424fe8ff01e1fb4cc99cad`.

This first implementation handles bundled text skills. It has no marketplace,
directory scanning, automatic installer, script execution, or support-resource
resolver. The Humanizer copy is self-contained; its source links are references,
not required supporting files. Add other skills deliberately when their needs
fit the runtime. The runtime does not mount or search the developer's home.

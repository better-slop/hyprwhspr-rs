# Dictation

Speech becomes text that the user can copy or insert into another application.

## Language

**Dictation**:
Speech captured to produce an editable transcript.

**Transcript**:
The text produced from a dictation, including any configured text corrections.

**Dictation session**:
One recording and its subsequent processing, ending with a result, a failure, or
cancellation.

**Activation**:
A request to begin, end, or cancel a dictation. A keyboard shortcut and an explicit
recording command are different ways to activate dictation.

**Delivery**:
The intentional handoff of a transcript to the clipboard or an editing target.

**Delivery intent**:
An internal request to copy, type, or paste. Dictation keeps the released
`auto_copy_clipboard` setting: true means paste, false means typing. Explicit
history-copy actions request copy independently of that setting.

**Copy**:
Place a transcript on the clipboard without inserting it into another application.

**Typing**:
Enter a transcript into the focused editing target without using the clipboard.

**Paste**:
Place a transcript on the clipboard and ask the focused application to paste it.

**Hold activation**:
A dictation whose recording lasts while its initiating shortcut remains held.

**Toggle activation**:
A dictation whose recording starts and ends through separate activation requests.

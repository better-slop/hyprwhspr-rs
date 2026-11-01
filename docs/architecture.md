# Dictation across native hosts

The architecture stack concentrates delivery, dictation sessions, and shortcut
activation in separate deep modules. Each interface hides the ordering and
lifetime rules that its callers previously had to coordinate. Native adapters
implement the effects that vary between hosts.

The domain terms are defined in [GLOSSARY.md](../GLOSSARY.md).

## Text delivery

One delivery module owns the choice and ordering of clipboard writes and native
input. Dictation and explicit history-copy actions use the same implementation.
Text normalization happens before delivery and remains independently reusable.

- Copy changes the clipboard without sending an insertion event.
- Typing sends text without accessing the clipboard.
- Paste writes the transcript to the clipboard and sends the selected paste
  shortcut. Copy and paste retain the transcript on the clipboard.
- A missing native capability is an error, not permission to change delivery
  mode. Select a usable adapter before side effects.
- A failure after possible insertion must not trigger another insertion attempt.
  The target may already contain some text.

Native event acceptance is not confirmation that an application changed its
document. Tests distinguish dispatch behavior from observed text in a real
editing target. Delivery cannot promise exactly-once insertion into arbitrary
applications without a target acknowledgment.

Safe clipboard restoration requires a separate native capability: bounded
snapshots of supported formats, ownership checks, and generation checks against
newer clipboard changes. Text-only snapshots cannot establish that capability.
The initial stack deliberately retains copied text instead of advertising lossy
clipboard preservation.

## Shared dictation sessions

Daemon shortcuts, recording commands, and interactive stdin should invoke the
same session implementation. Capture, processing, cancellation, and completion
must have one owner. Native hosts assemble capture, transcription, delivery,
status, and feedback adapters.

Processing must not block activation handling. A canceled or superseded result
must not later reach history or delivery. Native inference may continue after
its caller stops waiting; cancellation must fence the eventual result rather
than assume an aborted task has stopped every native thread.

The session interface is the test surface. Controlled adapters let tests exercise
the same state transitions and completion rules used by actual hosts without a
microphone, downloaded model, or compositor.

## Shortcut activation

The session consumes activation intent rather than Linux key codes or device
identifiers. The Linux adapter owns evdev discovery, udev reconciliation,
debouncing, and device churn. Its runtime must preserve end and cancellation
events even when the event queue is busy.

A native global-shortcut adapter should report its actual capabilities. Hold
release, modifier-only shortcuts, suppression, and synthetic-event recognition
are not identical across hosts. Runtime tests should drive the real dispatch
loop with a controlled adapter, rather than only testing device-state getters.

## Platform reach

Linux is the supported native host for this stack. A portable module structure
does not by itself establish macOS, Windows, mobile, or browser support.

- Wayland support depends on advertised compositor and portal capabilities, not
  only an environment variable. Hyprland IPC is one native adapter.
- X11 needs a real clipboard and injection implementation enabled in the build.
- macOS and Windows need native permission, clipboard, shortcut, input, and
  packaging adapters. The existing Windows prototype PR #116 is prior work to
  account for when implementing its host.
- Android needs an input-method host. iOS needs a containing application and
  custom keyboard with a supported recording and handoff design.
- Ordinary web pages can insert into their own editors and offer explicit copy;
  system-wide typing requires an authorized native companion.
- Encoding, model loading, and inference also need host-supported adapters.
  Desktop external processes cannot be assumed on mobile or in a browser.

The next native port should prove a second real host before widening the shared
interface. Keep configuration and policy shared; keep host facts inside adapters.

## Verification and release

Each stack entry receives focused regression tests. The integrated stack runs
tests, Clippy, documentation generation, a build without default features, and
`cargo build --release`. Changed Rust files are checked by rustfmt; unrelated
pre-existing formatting is outside the stack.

Native acceptance additionally records, transcribes, and inserts into a real
editor, terminal, and browser field. Include Unicode, non-US layouts, composition,
Enter and Tab, held modifiers, focus changes, permission loss, and clipboard
ownership changes where supported.

Release-plz remains the version owner. Add native build, acceptance, packaging,
and signing gates as each additional host ships; a Linux release alone does not
prove support for another host.

## Prior art

[Hex](https://github.com/anomalyco/hex/tree/caa29177be12c683fb75899b41123ee90ce77ff2)
demonstrates native desktop adapters, explicit capabilities, clipboard ownership
and generation checks, and a separate iOS keyboard host. Its paste orchestration
still differs between operating systems. The shared delivery module should
concentrate those policies instead of repeating them in every adapter.

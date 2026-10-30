# Coordinators speak only through hub tools

A coordinator's visible messages come from `post_message`, `reply` and `no_reply_needed`, not from its assistant text. With these tools, the assistant can reply inside a thread, or stay silent when a task update needs no comment, as Duet does. It posts one message for one reply: since the redesign of 2026-10-30, the prompt no longer asks for several short messages.

Do not change the hub to post a coordinator's assistant text directly. The hub posts the last assistant message only as a fallback, when a turn ends without any of the three calls.

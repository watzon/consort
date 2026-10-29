# Coordinators speak only through hub tools

A coordinator's visible messages come from `post_message`, `reply` and `no_reply_needed`, not from its assistant text. With these tools, the assistant can answer in several short messages, reply inside a thread, or stay silent when a task update needs no comment, as Duet does.

Do not change the hub to post a coordinator's assistant text directly. The hub posts the last assistant message only as a fallback, when a turn ends without any of the three calls.

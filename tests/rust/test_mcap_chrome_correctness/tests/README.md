# MCAP Chrome correctness E2E

This package is a production-disarmed Chrome-only boundary suite.
It uses the existing controlled MCAP Range fixture and does not install or test real remote-MCAP open, Store, query, seek, or playback.
Those release gates belong to MCAP-114 or a later real-capability installation.

The JavaScript orchestration covers same-origin and cross-origin legal Range responses, CORS preflight/exposure failures, CSP `connect-src`, malformed `Content-Range`, early EOF, overlong body, bounded infinite-body cancellation and abort, BYOB EOF probing, `Response.arrayBuffer()` avoidance, no Range-less GET traces, browser error redaction, and the current production-disarmed strict URL classification boundary.

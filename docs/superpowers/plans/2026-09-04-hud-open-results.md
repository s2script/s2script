# HUD open-result implementation plan

1. Confirm the existing native void-success versus rejection contract; preserve its public unchecked API.
2. Convert rejected HUD drives to errors and update class/text caches only on success.
3. Make modal paint collect drive errors; stage candidate state in `tryOpen`, publish only after show succeeds, and delegate legacy `open` to it.
4. Add result types and bound-view methods; route failed Menu opens to chat and release otherwise unused claims.
5. Add failure/retry and fallback tests over the shipped preludes. Run focused tests, the full JS gate, and diff checks.
6. Rebase onto the shared-capture slice, verify combined capture-failure behavior, and publish one reviewable PR against that branch. Do not merge or deploy.

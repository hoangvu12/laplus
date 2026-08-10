# 05 — ChatHeader action menu ignored pointer activation

**Status:** done

## Problem

The first real-window pinning walkthrough found that the ChatHeader title
rendered its accessible action label but did not open the menu under pointer
input. Source-level policy tests could not observe the broken trigger
composition, so pinning from the header was unreachable in the application.

## Resolution

Compose the tooltip trigger around a `MenuTrigger` with an explicit button
render target, matching the repository's working menu/tooltip controls. The
current bundle was rebuilt and the CDP walkthrough then opened the title menu,
pinned threads, and exercised lifecycle actions through it.

## Comments

Found and fixed during ticket 04 on 2026-08-10.

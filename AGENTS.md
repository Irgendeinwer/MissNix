# Project Mission: Local Music Release Tracker

## The Core Problem
People who maintain local music libraries (instead of relying on streaming platforms) lose out on automated "Release Radar" discovery. They have no effortless way to know when artists already sitting in their music folders drop new albums, EPs, or singles.

Your goal is to help design and build a **standalone desktop application** that bridges this gap: it turns a user's existing local music collection into an automated, visually appealing new-release feed.

---

## What the App Must Do (Core Workflow)

1. **Discover Artists from Local Music**
   - Scan one or more user-selected local music folders to identify the artists in their collection.
   - Do not rely solely on messy folder naming conventions; extract reliable artist identity from the audio files whenever possible, falling back gracefully when metadata is incomplete.
   - Eliminate tedious manual artist searching, while still letting users ignore, mute, or exclude specific artists (e.g., one-off compilation or soundtrack appearances) and optionally add artists manually.

2. **Persist Library & Tracking State**
   - Remember discovered artists, when they were last checked, known releases, and user interaction state (such as "unseen", "seen/dismissed", or "ignored") across restarts.

3. **Monitor for New Releases**
   - Check external music data sources (on demand and/or on a schedule) to find recent and upcoming releases from the tracked artists.
   - **Smart Local Deduplication:** Cross-check discovered releases against what the user already has in their local library so the app avoids reminding them about albums they already own.
   - Filter out noise where practical (such as duplicate regional/deluxe editions or unrelated compilation appearances).

4. **Present Results in a Clean Desktop GUI**
   - Display discovered releases in a responsive, visual desktop interface focused on cover art, artist names, release types (Album, EP, Single), and release dates.
   - Clicking a release should reveal rich details (full artwork, tracklist, and helpful actions like marking as seen, previewing if available, or opening external artist/release links).
   - Provide clear views for browsing unread drops, browsing all tracked artists, filtering/searching, and adjusting preferences.

---

## Design & User Experience Principles

- **Desktop-First Ergonomics:** Aim for a modern, cohesive, cover-art-centric aesthetic (clean tonal cards, dark-mode friendly, expressive visual hierarchy). However, **adapt it genuinely for desktop**—compact spacing, mouse hover/context interactions, keyboard navigation, and multi-pane layouts that make smart use of wide monitors instead of feeling like a blown-up phone app.
- **Fast & Non-Blocking:** Opening the app should feel instant. Library scanning and online lookups should run in the background without ever freezing the interface.
- **Resilient & Polite:** Handle offline states, ambiguous artist names (two artists sharing a name), missing artwork, and messy music folders gracefully without crashing. Avoid needlessly hammering external services.
- **Clean Separation of Concerns:** Keep the core logic (scanning, tracking state, release checking) clearly separated from the visual desktop interface so the project stays modular, testable, and easy to iterate on.

---

## How We Will Work Together
- Before writing code, help evaluate and propose a fitting architecture and technology choices tailored to these goals.
- Work iteratively in clear, testable milestones (e.g., reliable local discovery first, then release matching, then the visual interface).
- Keep code well-structured and maintainable, favoring clarity and reliability over unnecessary complexity.

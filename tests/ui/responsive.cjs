// End-to-end Slint layout checks. They use the production UI, copied to a
// temporary directory with read-only geometry probes added for assertions.
// Window events exercise the same pointer/key/resize dispatch as the backend.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const slint = require('slint-ui');

const repo = path.resolve(__dirname, '../..');
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'missnix-ui-'));
process.on('exit', () => fs.rmSync(scratch, { recursive: true, force: true }));
fs.cpSync(path.join(repo, 'ui'), path.join(scratch, 'ui'), { recursive: true });
fs.symlinkSync(path.join(repo, 'assets'), path.join(scratch, 'assets'), 'dir');

function replaceOnce(text, needle, replacement) {
    const at = text.indexOf(needle);
    assert(at >= 0 && text.indexOf(needle, at + needle.length) < 0, `Missing or ambiguous test anchor: ${needle}`);
    return text.slice(0, at) + replacement + text.slice(at + needle.length);
}

function nameBefore(text, marker, element, id) {
    const markerAt = text.indexOf(marker);
    assert(markerAt >= 0 && text.indexOf(marker, markerAt + marker.length) < 0, `Missing or ambiguous marker: ${marker}`);
    const at = text.lastIndexOf(element, markerAt);
    assert(at >= 0, `Missing ${element} before ${marker}`);
    return text.slice(0, at) + `${id} := ` + text.slice(at);
}

function nameAfter(text, marker, element, id) {
    const markerAt = text.indexOf(marker);
    assert(markerAt >= 0 && text.indexOf(marker, markerAt + marker.length) < 0, `Missing or ambiguous marker: ${marker}`);
    const at = text.indexOf(element, markerAt);
    assert(at >= 0, `Missing ${element} after ${marker}`);
    return text.slice(0, at) + `${id} := ` + text.slice(at);
}

function instrument(file, fn) {
    const filename = path.join(scratch, 'ui', file);
    fs.writeFileSync(filename, fn(fs.readFileSync(filename, 'utf8')));
}

instrument('components/primitives.slint', source => {
    source = replaceOnce(source, 'export component M3SearchField inherits Rectangle {', `export component M3SearchField inherits Rectangle {
    out property <Point> test-clear-position;`);
    source = replaceOnce(source, 'export component M3LabeledInput inherits VerticalLayout {', `export component M3LabeledInput inherits VerticalLayout {
    out property <Point> test-clear-position;`);
    source = nameBefore(source, 'accessible-name: "Clear search";', 'M3IconButton {', 'clear-search');
    source = nameBefore(source, 'accessible-name: "Clear input";', 'M3IconButton {', 'clear-input');
    for (const id of ['clear-search', 'clear-input']) {
        source = replaceOnce(source, `${id} := M3IconButton {`, `${id} := M3IconButton {
        init => { root.test-clear-position = self.absolute-position; }
        changed absolute-position => { root.test-clear-position = self.absolute-position; }`);
    }
    source = replaceOnce(source, 'export component M3ChipGrid inherits Rectangle {', `export component M3ChipGrid inherits Rectangle {
    out property <int> test-columns: root.columns;
    out property <int> test-rows: root.rows;`);
    source = replaceOnce(source, 'export component M3LabeledChipGrid inherits Rectangle {', `export component M3LabeledChipGrid inherits Rectangle {
    out property <Point> test-chip-position: chips.absolute-position;
    out property <int> test-columns: chips.test-columns;
    out property <length> test-cell-width: chips.cell-width;`);
    return replaceOnce(source, 'export component AccentSwatchGrid inherits Rectangle {', `export component AccentSwatchGrid inherits Rectangle {
    out property <int> test-columns: root.columns;`);
});

instrument('components/library-widgets.slint', source => replaceOnce(source,
    'export component SearchCandidateRow inherits Rectangle {', `export component SearchCandidateRow inherits Rectangle {
    out property <Point> test-track-position: track-button.absolute-position;
    out property <length> test-track-width: track-button.width;`));

instrument('components/media.slint', source => {
    source = replaceOnce(source, 'export component PreviewMiniPlayer inherits Rectangle {', `export component PreviewMiniPlayer inherits Rectangle {
    out property <Point> test-play-position: play-button.absolute-position;
    out property <Point> test-stop-position: stop-button.absolute-position;
    out property <Point> test-volume-position: volume-button.absolute-position;
    out property <Point> test-seek-position: seek-area.absolute-position;
    out property <length> test-seek-width: seek-area.width;`);
    source = nameBefore(source, 'filled-primary: true;', 'M3IconButton {', 'play-button');
    return nameBefore(source, 'accessible-name: root.volume-pct == 0', 'M3IconButton {', 'volume-button');
});

instrument('views/nav-rail.slint', source => {
    source = replaceOnce(source, 'export component NavRail inherits Rectangle {', `export component NavRail inherits Rectangle {
    out property <Point> test-menu-position;
    out property <bool> test-menu-focused;
    out property <Point> test-artists-position;
    out property <bool> test-artists-focused;
    out property <Point> test-cancel-position;
    out property <length> test-scroll-maximum;`);
    source = nameBefore(source, 'accessible-name: "Open navigation and status";', 'M3IconButton {', 'compact-menu');
    source = nameBefore(source, 'label: "Tracked Artists";', 'NavRailItem {', 'full-artists');
    source = nameBefore(source, 'text: "Cancel";', 'M3Button {', 'cancel-button');
    source = replaceOnce(source, 'compact-menu := M3IconButton {', `compact-menu := M3IconButton {
        init => { root.test-menu-position = self.absolute-position; }
        changed absolute-position => { root.test-menu-position = self.absolute-position; }
        changed has-focus => { root.test-menu-focused = self.has-focus; }`);
    source = replaceOnce(source, 'full-artists := NavRailItem {', `full-artists := NavRailItem {
        init => { root.test-artists-position = self.absolute-position; }
        changed absolute-position => { root.test-artists-position = self.absolute-position; }
        changed has-focus => { root.test-artists-focused = self.has-focus; }`);
    source = replaceOnce(source, 'cancel-button := M3Button {', `cancel-button := M3Button {
        init => { root.test-cancel-position = self.absolute-position; }
        changed absolute-position => { root.test-cancel-position = self.absolute-position; }`);
    return replaceOnce(source, 'rail-scroll := ScrollView {', `rail-scroll := ScrollView {
        function test-update() { root.test-scroll-maximum = max(0px, self.content-height - self.visible-height); }
        init => { self.test-update(); }
        changed content-height => { self.test-update(); }
        changed visible-height => { self.test-update(); }`);
});

instrument('views/inspector-drawer.slint', source => {
    source = replaceOnce(source, 'export component InspectorDrawer inherits Rectangle {', `export component InspectorDrawer inherits Rectangle {
    out property <length> test-slide: root.slide-offset;
    out property <Point> test-release-close: release-close.absolute-position;
    out property <Point> test-artist-close: artist-close.absolute-position;
    out property <Point> test-seen-position: seen-button.absolute-position;
    out property <Point> test-mbid-position: mbid-input.absolute-position;
    out property <Point> test-mbid-clear-position: mbid-input.test-clear-position;
    out property <Point> test-save-position: save-button.absolute-position;
    out property <length> test-artist-scroll: -art-scroll.content-y;
    out property <length> test-artist-scroll-maximum: max(0px, art-scroll.content-height - art-scroll.visible-height);
    out property <length> test-release-horizontal-maximum: max(0px, rel-scroll.content-width - rel-scroll.visible-width);
    out property <length> test-release-horizontal-offset: -rel-scroll.content-x;
    out property <Point> test-release-bar-position: release-bar.absolute-position;
    out property <length> test-release-bar-width: release-bar.width;
    out property <length> test-artist-horizontal-maximum: max(0px, art-scroll.content-width - art-scroll.visible-width);
    out property <length> test-artist-horizontal-offset: -art-scroll.content-x;
    out property <Point> test-artist-bar-position: artist-bar.absolute-position;
    out property <length> test-artist-bar-width: artist-bar.width;
    out property <length> test-badge-maximum: max(0px, badge-scroll.content-width - badge-scroll.visible-width);
    out property <length> test-badge-offset: -badge-scroll.content-x;
    out property <Point> test-badge-bar-position: badge-bar.absolute-position;
    out property <length> test-badge-bar-width: badge-bar.width;
    public function test-scroll-artist(value: length) { art-scroll.content-y = -value; }`);
    source = nameAfter(source, '// MODE 1: RELEASE DETAILS INSPECTOR', 'M3IconButton {', 'release-close');
    source = nameAfter(source, '// MODE 2: ARTIST INSPECTOR', 'M3IconButton {', 'artist-close');
    source = nameBefore(source, 'text: root.selected-release.unseen ? "Mark as Seen"', 'M3Button {', 'seen-button');
    source = nameBefore(source, 'label: "MusicBrainz Artist MBID";', 'M3LabeledInput {', 'mbid-input');
    source = nameBefore(source, 'text: "Save Provider IDs";', 'M3Button {', 'save-button');
    source = nameAfter(source, 'badge-scroll := ScrollView {', 'M3HorizontalScrollBar {', 'badge-bar');
    source = nameBefore(source, 'value: -rel-scroll.content-x;', 'M3HorizontalScrollBar {', 'release-bar');
    return nameBefore(source, 'value: -art-scroll.content-x;', 'M3HorizontalScrollBar {', 'artist-bar');
});

instrument('views/radar-view.slint', source => {
    source = replaceOnce(source, 'export component RadarView inherits Rectangle {', `export component RadarView inherits Rectangle {
    out property <length> test-grid-width;
    out property <length> test-grid-height;
    out property <length> test-card-width;
    out property <length> test-card-height;
    out property <int> test-columns;
    out property <length> test-grid-scroll;
    out property <Point> test-grid-position;
    out property <Point> test-search-position: feed-search-input.absolute-position;
    out property <Point> test-search-clear-position: feed-search-input.test-clear-position;
    out property <length> test-page-height: page-scroll.visible-height;
    out property <length> test-page-maximum: max(0px, page-scroll.content-height - page-scroll.visible-height);
    out property <length> test-page-scroll: -page-scroll.content-y;
    out property <length> test-chip-maximum: max(0px, chip-scroll.content-width - chip-scroll.visible-width);
    out property <length> test-chip-scroll: -chip-scroll.content-x;
    out property <Point> test-chip-bar-position: radar-chip-bar.absolute-position;
    out property <length> test-chip-bar-width: radar-chip-bar.width;
    out property <Point> test-singles-position: singles-chip.absolute-position;
    out property <length> test-singles-width: singles-chip.width;
    out property <Point> test-empty-card-position;
    out property <length> test-empty-card-width;
    out property <Point> test-empty-check-position;
    out property <length> test-empty-check-width;
    out property <Point> test-empty-reset-position;
    out property <length> test-empty-reset-width;
    out property <Point> test-empty-add-position;
    out property <length> test-empty-add-width;
    public function test-scroll-page(value: length) { page-scroll.content-y = -value; }
    public function test-scroll-chips(value: length) { chip-scroll.content-x = -value; }`);
    source = replaceOnce(source, 'feed-scroll := ScrollView {', `feed-scroll := ScrollView {
        function test-update() {
            root.test-grid-width = self.visible-width;
            root.test-grid-height = self.visible-height;
            root.test-card-width = self.card-w;
            root.test-card-height = self.card-h;
            root.test-columns = self.cols;
            root.test-grid-scroll = -self.content-y;
            root.test-grid-position = self.absolute-position;
        }
        init => { self.test-update(); }
        changed visible-width => { self.test-update(); }
        changed visible-height => { self.test-update(); }
        changed card-w => { self.test-update(); }
        changed card-h => { self.test-update(); }
        changed cols => { self.test-update(); }
        changed content-y => { self.test-update(); }
        changed absolute-position => { self.test-update(); }`);
    source = replaceOnce(source, 'empty-card := Rectangle {', `empty-card := Rectangle {
        function test-update() {
            root.test-empty-card-position = self.absolute-position;
            root.test-empty-card-width = self.width;
        }
        init => { self.test-update(); }
        changed absolute-position => { self.test-update(); }
        changed width => { self.test-update(); }`);
    for (const [id, property] of [['check-action', 'check'], ['reset-action', 'reset']]) {
        source = replaceOnce(source, `${id} := M3Button {`, `${id} := M3Button {
        function test-update() {
            root.test-empty-${property}-position = self.absolute-position;
            root.test-empty-${property}-width = self.width;
        }
        init => { self.test-update(); }
        changed absolute-position => { self.test-update(); }
        changed width => { self.test-update(); }`);
    }
    source = nameBefore(source, 'label: "Singles";', 'M3FilterChip {', 'singles-chip');
    source = nameBefore(source, 'value: -chip-scroll.content-x;', 'M3HorizontalScrollBar {', 'radar-chip-bar');
    source = nameBefore(source, 'text: "Add Music Folder";', 'M3Button {', 'add-folder-button');
    return replaceOnce(source, 'add-folder-button := M3Button {', `add-folder-button := M3Button {
        function test-update() {
            root.test-empty-add-position = self.absolute-position;
            root.test-empty-add-width = self.width;
        }
        init => { self.test-update(); }
        changed absolute-position => { self.test-update(); }
        changed width => { self.test-update(); }`);
});

instrument('views/artists-view.slint', source => {
    source = replaceOnce(source, 'export component ArtistsView inherits Rectangle {', `export component ArtistsView inherits Rectangle {
    out property <length> test-page-height: page-scroll.visible-height;
    out property <length> test-page-maximum: max(0px, page-scroll.content-height - page-scroll.visible-height);
    out property <length> test-page-scroll: -page-scroll.content-y;
    out property <Point> test-candidates-position;
    out property <length> test-candidates-width;
    out property <length> test-candidates-height;
    out property <length> test-candidates-maximum;
    out property <length> test-candidates-scroll;
    out property <Point> test-first-track-position;
    out property <length> test-first-track-width;
    out property <Point> test-first-row-position;
    out property <length> test-first-row-width;
    out property <length> test-first-row-height;
    out property <Point> test-filter-position: artist-filter-bar.absolute-position;
    out property <length> test-chip-maximum: max(0px, artist-chip-scroll.content-width - artist-chip-scroll.visible-width);
    out property <length> test-chip-offset: -artist-chip-scroll.content-x;
    out property <Point> test-chip-bar-position: artist-chip-bar.absolute-position;
    out property <length> test-chip-bar-width: artist-chip-bar.width;
    out property <Point> test-manual-chip-position: manual-chip.absolute-position;
    out property <length> test-manual-chip-width: manual-chip.width;
    public function test-scroll-page(value: length) { page-scroll.content-y = -value; }`);
    source = nameBefore(source, 'label: "Manual";', 'M3FilterChip {', 'manual-chip');
    source = nameBefore(source, 'value: -artist-chip-scroll.content-x;', 'M3HorizontalScrollBar {', 'artist-chip-bar');
    // The list lives inside two conditionals. Forward its reactive geometry
    // from that scope instead of referencing its private id from the root.
    source = replaceOnce(source, 'candidates-list := ListView {', `candidates-list := ListView {
        function test-update() {
            root.test-candidates-position = self.absolute-position;
            root.test-candidates-width = self.visible-width;
            root.test-candidates-height = self.visible-height;
            root.test-candidates-maximum = max(0px, self.content-height - self.visible-height);
            root.test-candidates-scroll = -self.content-y;
        }
        init => { self.test-update(); }
        changed absolute-position => { self.test-update(); }
        changed visible-width => { self.test-update(); }
        changed visible-height => { self.test-update(); }
        changed content-height => { self.test-update(); }
        changed content-y => { self.test-update(); }`);
    return replaceOnce(source, 'for cand in root.artist-search-candidates : VerticalLayout {\n                                    padding-bottom: 6px;\n                                    SearchCandidateRow {', `for cand in root.artist-search-candidates : VerticalLayout {
                                    padding-bottom: 6px;
                                    SearchCandidateRow {
        function test-update() {
            if cand.index == 0 {
                root.test-first-track-position = self.test-track-position;
                root.test-first-track-width = self.test-track-width;
                root.test-first-row-position = self.absolute-position;
                root.test-first-row-width = self.width;
                root.test-first-row-height = self.height;
            }
        }
        init => { self.test-update(); }
        changed test-track-position => { self.test-update(); }
        changed width => { self.test-update(); }
        changed height => { self.test-update(); }
        changed absolute-position => { self.test-update(); }`);
});

instrument('views/settings-view.slint', source => {
    source = replaceOnce(source, 'export component SettingsView inherits Rectangle {', `export component SettingsView inherits Rectangle {
    out property <length> test-visible-width: settings-scroll.visible-width;
    out property <length> test-content-width: settings-scroll.content-width;
    out property <length> test-content-x: -settings-scroll.content-x;
    out property <Point> test-bar-position: settings-bar.absolute-position;
    out property <length> test-bar-width: settings-bar.width;
    out property <length> test-scroll: -settings-scroll.content-y;
    out property <length> test-scroll-maximum: max(0px, settings-scroll.content-height - settings-scroll.visible-height);
    out property <length> test-column-width: settings-column.width;
    out property <length> test-column-x: settings-column.x;
    out property <Point> test-scale-position: scale-grid.absolute-position;
    out property <length> test-scale-width: scale-grid.width;
    out property <length> test-scale-cell: scale-grid.cell-width;
    out property <int> test-scale-columns: scale-grid.test-columns;
    out property <int> test-scale-rows: scale-grid.test-rows;
    out property <Point> test-swatch-position: swatch-grid.absolute-position;
    out property <int> test-swatch-columns: swatch-grid.test-columns;
    out property <Point> test-density-position: density-grid.test-chip-position;
    out property <length> test-density-cell: density-grid.test-cell-width;
    out property <int> test-density-columns: density-grid.test-columns;
    out property <Point> test-discovery-position: discovery-grid.absolute-position;
    out property <length> test-discovery-cell: discovery-grid.cell-width;
    out property <int> test-discovery-columns: discovery-grid.test-columns;
    out property <Point> test-add-path-position: add-path.absolute-position;
    out property <Point> test-browse-position: browse.absolute-position;
    public function test-scroll-vertical(value: length) { settings-scroll.content-y = -value; }
    public function test-scroll-horizontal(value: length) { settings-scroll.content-x = -value; }`);
    source = replaceOnce(source, '            VerticalLayout {\n                min-width: 0px;\n                max-width: 900px;', '            settings-column := VerticalLayout {\n                min-width: 0px;\n                max-width: 900px;');
    source = nameBefore(source, 'labels: ["90%", "100%",', 'M3ChipGrid {', 'scale-grid');
    source = nameBefore(source, 'label: "Release Cover Grid Size:";', 'M3LabeledChipGrid {', 'density-grid');
    source = nameBefore(source, 'selected-index: root.pref-accent-preset;', 'AccentSwatchGrid {', 'swatch-grid');
    return nameBefore(source, 'value: -settings-scroll.content-x;', 'M3HorizontalScrollBar {', 'settings-bar');
});

instrument('app-window.slint', source => {
    source = replaceOnce(source, 'export component AppWindow inherits Window {', `export component AppWindow inherits Window {
    out property <length> test-width: root.width;
    out property <length> test-height: root.height;
    out property <bool> test-overlay: root.overlay-inspector;
    out property <bool> test-compact: root.compact-navigation;
    out property <bool> test-navigation-expanded: root.navigation-expanded;
    out property <length> test-navigation-width: root.navigation-width;
    out property <length> test-center-width: center.width;
    out property <length> test-drawer-x: drawer.x;
    out property <length> test-drawer-width: drawer.width;
    out property <length> test-drawer-slide: drawer.test-slide;
    out property <Point> test-release-close: drawer.test-release-close;
    out property <Point> test-artist-close: drawer.test-artist-close;
    out property <Point> test-seen-position: drawer.test-seen-position;
    out property <Point> test-mbid-position: drawer.test-mbid-position;
    out property <Point> test-mbid-clear-position: drawer.test-mbid-clear-position;
    out property <Point> test-save-position: drawer.test-save-position;
    out property <length> test-artist-inspector-scroll: drawer.test-artist-scroll;
    out property <length> test-artist-inspector-scroll-maximum: drawer.test-artist-scroll-maximum;
    out property <length> test-release-horizontal-maximum: drawer.test-release-horizontal-maximum;
    out property <length> test-release-horizontal-offset: drawer.test-release-horizontal-offset;
    out property <Point> test-release-bar-position: drawer.test-release-bar-position;
    out property <length> test-release-bar-width: drawer.test-release-bar-width;
    out property <length> test-artist-horizontal-maximum: drawer.test-artist-horizontal-maximum;
    out property <length> test-artist-horizontal-offset: drawer.test-artist-horizontal-offset;
    out property <Point> test-artist-bar-position: drawer.test-artist-bar-position;
    out property <length> test-artist-bar-width: drawer.test-artist-bar-width;
    out property <length> test-badge-maximum: drawer.test-badge-maximum;
    out property <length> test-badge-offset: drawer.test-badge-offset;
    out property <Point> test-badge-bar-position: drawer.test-badge-bar-position;
    out property <length> test-badge-bar-width: drawer.test-badge-bar-width;
    public function test-scroll-artist-inspector(value: length) { drawer.test-scroll-artist(value); }

    out property <Point> test-menu-position: nav.test-menu-position;
    out property <bool> test-menu-focused: nav.test-menu-focused;
    out property <Point> test-nav-artists-position: nav.test-artists-position;
    out property <bool> test-nav-artists-focused: nav.test-artists-focused;
    out property <Point> test-nav-cancel-position: nav.test-cancel-position;
    out property <bool> test-nav-cancel-focused: nav.cancel-has-focus;
    out property <length> test-nav-scroll-maximum: nav.test-scroll-maximum;

    out property <Point> test-player-position: player.absolute-position;
    out property <length> test-player-width: player.width;
    out property <length> test-player-height: player.height;
    out property <Point> test-play-position: player.test-play-position;
    out property <Point> test-stop-position: player.test-stop-position;
    out property <Point> test-volume-position: player.test-volume-position;
    out property <Point> test-seek-position: player.test-seek-position;
    out property <length> test-seek-width: player.test-seek-width;

    out property <length> test-grid-width: radar-view.test-grid-width;
    out property <length> test-grid-height: radar-view.test-grid-height;
    out property <length> test-card-width: radar-view.test-card-width;
    out property <length> test-card-height: radar-view.test-card-height;
    out property <int> test-columns: radar-view.test-columns;
    out property <length> test-grid-scroll: radar-view.test-grid-scroll;
    out property <Point> test-grid-position: radar-view.test-grid-position;
    out property <Point> test-radar-search-position: radar-view.test-search-position;
    out property <Point> test-radar-search-clear-position: radar-view.test-search-clear-position;
    out property <length> test-radar-page-height: radar-view.test-page-height;
    out property <length> test-radar-page-maximum: radar-view.test-page-maximum;
    out property <length> test-radar-page-scroll: radar-view.test-page-scroll;
    out property <length> test-radar-chip-maximum: radar-view.test-chip-maximum;
    out property <length> test-radar-chip-scroll: radar-view.test-chip-scroll;
    out property <Point> test-radar-chip-bar-position: radar-view.test-chip-bar-position;
    out property <length> test-radar-chip-bar-width: radar-view.test-chip-bar-width;
    out property <Point> test-singles-position: radar-view.test-singles-position;
    out property <length> test-singles-width: radar-view.test-singles-width;
    out property <Point> test-empty-card-position: radar-view.test-empty-card-position;
    out property <length> test-empty-card-width: radar-view.test-empty-card-width;
    out property <Point> test-empty-check-position: radar-view.test-empty-check-position;
    out property <length> test-empty-check-width: radar-view.test-empty-check-width;
    out property <Point> test-empty-reset-position: radar-view.test-empty-reset-position;
    out property <length> test-empty-reset-width: radar-view.test-empty-reset-width;
    out property <Point> test-empty-add-position: radar-view.test-empty-add-position;
    out property <length> test-empty-add-width: radar-view.test-empty-add-width;
    public function test-scroll-radar-page(value: length) { radar-view.test-scroll-page(value); }
    public function test-scroll-radar-chips(value: length) { radar-view.test-scroll-chips(value); }

    out property <length> test-artists-page-height: artists-view.test-page-height;
    out property <length> test-artists-page-maximum: artists-view.test-page-maximum;
    out property <length> test-artists-page-scroll: artists-view.test-page-scroll;
    out property <Point> test-candidates-position: artists-view.test-candidates-position;
    out property <length> test-candidates-width: artists-view.test-candidates-width;
    out property <length> test-candidates-height: artists-view.test-candidates-height;
    out property <length> test-candidates-maximum: artists-view.test-candidates-maximum;
    out property <length> test-candidates-scroll: artists-view.test-candidates-scroll;
    out property <Point> test-first-track-position: artists-view.test-first-track-position;
    out property <length> test-first-track-width: artists-view.test-first-track-width;
    out property <Point> test-first-row-position: artists-view.test-first-row-position;
    out property <length> test-first-row-width: artists-view.test-first-row-width;
    out property <length> test-first-row-height: artists-view.test-first-row-height;
    out property <Point> test-artist-filter-position: artists-view.test-filter-position;
    out property <length> test-artist-chip-maximum: artists-view.test-chip-maximum;
    out property <length> test-artist-chip-offset: artists-view.test-chip-offset;
    out property <Point> test-artist-chip-bar-position: artists-view.test-chip-bar-position;
    out property <length> test-artist-chip-bar-width: artists-view.test-chip-bar-width;
    out property <Point> test-manual-chip-position: artists-view.test-manual-chip-position;
    out property <length> test-manual-chip-width: artists-view.test-manual-chip-width;
    public function test-scroll-artists-page(value: length) { artists-view.test-scroll-page(value); }

    out property <length> test-settings-visible-width: settings-view.test-visible-width;
    out property <length> test-settings-content-width: settings-view.test-content-width;
    out property <length> test-settings-content-x: settings-view.test-content-x;
    out property <Point> test-settings-bar-position: settings-view.test-bar-position;
    out property <length> test-settings-bar-width: settings-view.test-bar-width;
    out property <length> test-settings-scroll: settings-view.test-scroll;
    out property <length> test-settings-scroll-maximum: settings-view.test-scroll-maximum;
    out property <length> test-settings-column-width: settings-view.test-column-width;
    out property <length> test-settings-column-x: settings-view.test-column-x;
    out property <Point> test-scale-position: settings-view.test-scale-position;
    out property <length> test-scale-width: settings-view.test-scale-width;
    out property <length> test-scale-cell: settings-view.test-scale-cell;
    out property <int> test-scale-columns: settings-view.test-scale-columns;
    out property <int> test-scale-rows: settings-view.test-scale-rows;
    out property <Point> test-swatch-position: settings-view.test-swatch-position;
    out property <int> test-swatch-columns: settings-view.test-swatch-columns;
    out property <Point> test-density-position: settings-view.test-density-position;
    out property <length> test-density-cell: settings-view.test-density-cell;
    out property <int> test-density-columns: settings-view.test-density-columns;
    out property <Point> test-discovery-position: settings-view.test-discovery-position;
    out property <length> test-discovery-cell: settings-view.test-discovery-cell;
    out property <int> test-discovery-columns: settings-view.test-discovery-columns;
    out property <Point> test-add-path-position: settings-view.test-add-path-position;
    out property <Point> test-browse-position: settings-view.test-browse-position;
    public function test-scroll-settings(value: length) { settings-view.test-scroll-vertical(value); }
    public function test-scroll-settings-horizontal(value: length) { settings-view.test-scroll-horizontal(value); }`);
    source = replaceOnce(source, '            VerticalLayout {\n                // Only a docked inspector', '            center := VerticalLayout {\n                // Only a docked inspector');
    source = replaceOnce(source, '                PreviewMiniPlayer {', '                player := PreviewMiniPlayer {');
    return replaceOnce(source, '                    SettingsView {', '                    settings-view := SettingsView {');
});

const ui = slint.loadFile(path.join(scratch, 'ui/app-window.slint'), { style: 'material-dark', quiet: true });
const releases = Array.from({ length: 120 }, (_, i) => ui.ReleaseCardData({
    id: i + 1, artist_id: 1, title: `Release ${i + 1}`, artist_name: 'Example Artist',
    release_type: 'ALBUM', monogram: 'E', unseen: true, release_date: '2026-11-27',
}));
const artists = Array.from({ length: 20 }, (_, i) => ui.ArtistRowData({
    id: i + 1, name: `Example Artist ${i + 1}`, status: 'tracked', monogram: 'E',
    source_label: 'Local library', release_count: 8,
}));
const candidates = Array.from({ length: 12 }, (_, i) => ui.SearchCandidateRowData({
    index: i, name: `Candidate ${i + 1}`, subtitle: 'Example result', source_badge: 'Deezer',
}));
const app = new ui.AppWindow({ releases, artists, filtered_unseen_count: releases.length });
const events = { selections: [], closes: 0, seen: [], pause: 0, seek: [], volume: [], stopped: 0,
    scale: [], preferences: 0, scanPreferences: 0, added: [], browse: 0, candidates: [], saved: [], cancel: 0,
    checks: 0, feedFilters: 0 };

app.select_release = id => { events.selections.push(id); app.selected_release = releases[id - 1]; app.selected_release_id = id; app.inspector_mode = 1; };
app.close_inspector = () => { events.closes++; app.inspector_mode = 0; app.selected_release_id = 0; app.selected_artist_id = 0; };
app.navigate = view => { app.active_view = view; app.inspector_mode = 0; app.selected_release_id = 0; app.selected_artist_id = 0; };
app.toggle_release_seen = id => events.seen.push(id);
app.toggle_preview_pause = () => events.pause++;
app.seek_preview = value => events.seek.push(value);
app.set_preview_volume = value => events.volume.push(value);
app.stop_preview = () => { events.stopped++; app.preview_active = false; };
app.ui_scale_changed = value => { events.scale.push(value); app.pref_ui_scale_pct = value; };
app.preferences_changed = () => events.preferences++;
app.scan_preferences_changed = () => events.scanPreferences++;
app.add_folder_path = value => events.added.push(value);
app.browse_for_folder = () => events.browse++;
app.add_search_candidate = value => {
    events.candidates.push(value);
    app.artist_search_candidates = [];
    app.show_add_artist_bar = false;
};
app.save_artist_provider_ids = (id, mbid, deezer) => events.saved.push([id, mbid, deezer]);
app.cancel_background_task = () => events.cancel++;
app.check_all_releases = () => events.checks++;
app.feed_filters_changed = () => events.feedFilters++;

// Leave extra time for the software renderer on shared CI runners.
const pause = ms => new Promise(resolve => setTimeout(resolve, ms * (process.env.CI ? 2 : 1)));
async function resize(width, height) {
    app.window.logicalSize = { width, height };
    await pause(180);
}
async function click(position, dx = 0, dy = 0) {
    const p = { x: position.x + dx, y: position.y + dy };
    assert(Number.isFinite(p.x) && Number.isFinite(p.y), 'pointer position must be finite');
    app.window.dispatchEvent({ type: 'pointer-moved', position: p });
    app.window.dispatchEvent({ type: 'pointer-pressed', position: p, button: 'left' });
    app.window.dispatchEvent({ type: 'pointer-released', position: p, button: 'left' });
    await pause(90);
}
async function key(text) {
    app.window.dispatchEvent({ type: 'key-pressed', text });
    app.window.dispatchEvent({ type: 'key-released', text });
    await pause(60);
}
function closeEnough(a, b, message) { assert(Math.abs(a - b) < 1.5, `${message}: ${a} vs ${b}`); }
function visible(point, width = 32, height = 32) {
    assert(point.x >= -1 && point.y >= -1 && point.x + width <= app.test_width + 1 && point.y + height <= app.test_height + 1,
        `control outside window: ${point.x},${point.y} ${width}x${height} in ${app.test_width}x${app.test_height}`);
}
function selectedCardVisible(index) {
    const top = 20 + Math.floor(index / app.test_columns) * (app.test_card_height + 14);
    assert(top >= app.test_grid_scroll - 1, 'selected card must not be above the grid viewport');
    if (app.test_card_height <= app.test_grid_height - 24) {
        assert(top + app.test_card_height <= app.test_grid_scroll + app.test_grid_height + 1,
            'selected card must be visible after grid reflow');
    }
}
function gridClick(position, cell, columns, index, rowHeight = 40) {
    return click(position, (index % columns) * (cell + 8) + cell / 2,
        Math.floor(index / columns) * rowHeight + 16);
}
async function revealSettings(point) {
    const target = app.test_settings_scroll + point.y - 100;
    app.test_scroll_settings(Math.max(0, Math.min(app.test_settings_scroll_maximum, target)));
    await pause(100);
}

(async () => {
    app.window.logicalSize = { width: 1400, height: 880 };
    app.show();
    slint.runEventLoop();
    await pause(350);

    // Wide windows retain a docked inspector and a one-step grid reflow.
    assert.equal(app.test_overlay, false);
    assert.equal(app.test_compact, false);
    assert.equal(app.test_columns, 5);
    const wideGrid = app.test_grid_width;
    app.window.dispatchEvent({ type: 'pointer-scrolled', position: { x: 500, y: 500 }, deltaX: 0, deltaY: -1100 });
    await pause(100);
    assert(app.test_grid_scroll > 0);
    app.selected_release = releases[25];
    app.selected_release_id = 26;
    app.inspector_mode = 1;
    await pause(35);
    const earlyGrid = app.test_grid_width;
    await pause(220);
    closeEnough(app.test_drawer_x + app.test_drawer_width, app.test_width, 'docked right edge');
    closeEnough(app.test_drawer_width, 404, 'docked inspector width');
    closeEnough(app.test_center_width, app.test_width - 244 - 404, 'docked main width');
    closeEnough(earlyGrid, app.test_grid_width, 'grid must not repeatedly reflow during slide');
    assert(wideGrid - app.test_grid_width > 400);
    assert.equal(app.test_columns, 3);
    selectedCardVisible(25);
    assert(app.test_drawer_slide < 1, 'drawer finishes its slide');
    visible(app.test_release_close);
    await click(app.test_release_close, 16, 16);
    assert.equal(app.inspector_mode, 0);
    console.log('PASS docked inspector, selected-card reveal, and close');

    // At intermediate widths the overlay does not steal the main view's space.
    await resize(1100, 880);
    assert.equal(app.test_overlay, true);
    assert.equal(app.test_compact, false);
    const overlayCenter = app.test_center_width;
    const overlayGrid = app.test_grid_width;
    app.selected_release = releases[10]; app.selected_release_id = 11; app.inspector_mode = 1;
    await pause(200);
    closeEnough(app.test_center_width, overlayCenter, 'overlay leaves center width unchanged');
    closeEnough(app.test_grid_width, overlayGrid, 'overlay leaves grid width unchanged');
    closeEnough(app.test_drawer_x + app.test_drawer_width, app.test_width, 'overlay right edge');
    await resize(1400, 880);
    assert.equal(app.inspector_mode, 1);
    assert.equal(app.test_overlay, false, 'inspector docks when the window grows');
    closeEnough(app.test_center_width, app.test_width - 244 - 404, 'resized docked center');
    selectedCardVisible(10);
    await resize(1100, 880);
    assert.equal(app.inspector_mode, 1);
    assert.equal(app.test_overlay, true, 'inspector overlays again when the window shrinks');
    closeEnough(app.test_center_width, overlayCenter, 'resized overlay center');
    await click({ x: 100, y: 300 }); // scrim, not the navigation behind it
    assert.equal(app.inspector_mode, 0);
    app.selected_release_id = 11; app.inspector_mode = 1;
    await pause(200);
    await key('\x1b');
    assert.equal(app.inspector_mode, 0, 'Escape closes the overlay');
    await key('/');
    await key('x');
    assert.equal(app.feed_search_text, 'x');
    visible(app.test_radar_search_clear_position, 24, 24);
    await click(app.test_radar_search_clear_position, 12, 12);
    assert.equal(app.feed_search_text, '');
    await key('y');
    assert.equal(app.feed_search_text, 'y', 'typing continues after clearing search');
    app.feed_search_text = '';
    console.log('PASS overlay width, dismissal, and search focus');

    // Reproduce the actual in-app scaling path: keep the physical window size,
    // then dispatch a larger effective scale and the corresponding logical size.
    await resize(1200, 900);
    const physical = app.window.physicalSize;
    app.selected_release = releases[0]; app.selected_release_id = 1; app.inspector_mode = 1;
    app.window.dispatchEvent({ type: 'scale-factor-changed', scaleFactor: 1.75 });
    app.window.dispatchEvent({ type: 'resized', size: { width: physical.width / 1.75, height: physical.height / 1.75 } });
    app.window.requestRedraw();
    await pause(250);
    closeEnough(app.test_width, physical.width / 1.75, 'scaled logical width');
    assert.equal(app.test_compact, true);
    assert.equal(app.test_overlay, true);
    closeEnough(app.test_navigation_width, 72, 'compact rail width');
    assert(app.test_center_width > 600, 'main content must not be squeezed by the inspector');
    closeEnough(app.test_drawer_width, 404, 'scaled inspector width');
    closeEnough(app.test_drawer_x + app.test_drawer_width, app.test_width, 'scaled inspector right edge');
    visible(app.test_release_close);
    visible(app.test_seen_position, 100, 32);
    await click(app.test_seen_position, 35, 16);
    assert.deepEqual(events.seen, [1]);
    await click(app.test_release_close, 16, 16);
    assert.equal(app.inspector_mode, 0);
    app.selected_release_id = 1; app.inspector_mode = 1;
    app.window.dispatchEvent({ type: 'scale-factor-changed', scaleFactor: 2 });
    app.window.dispatchEvent({ type: 'resized', size: { width: physical.width / 2, height: physical.height / 2 } });
    app.window.requestRedraw();
    await pause(230);
    closeEnough(app.test_width, physical.width / 2, '200% logical width');
    closeEnough(app.test_center_width, app.test_width - 72, '200% inspector does not squeeze center');
    closeEnough(app.test_drawer_x + app.test_drawer_width, app.test_width, '200% inspector right edge');
    visible(app.test_release_close);
    await key('\x1b');
    assert.equal(app.inspector_mode, 0);
    console.log('PASS physical-size-preserving 175% and 200% scale changes');

    // Restore a normal scale before testing a truly small floating window.
    app.window.dispatchEvent({ type: 'scale-factor-changed', scaleFactor: 1 });
    await resize(480, 400);
    assert.equal(app.test_compact, true);
    const menu = app.test_menu_position;
    visible(menu, 40, 40);
    // Icon-only compact navigation remains keyboard reachable and operable.
    for (let i = 0; i < 60 && !app.test_menu_focused; i++) await key('\t');
    assert(app.test_menu_focused, 'Tab reaches the compact navigation menu');
    await key('\n');
    assert.equal(app.test_navigation_expanded, true, 'Enter opens compact navigation');
    for (let i = 0; i < 100 && !app.test_nav_artists_focused; i++) await key('\t');
    assert(app.test_nav_artists_focused, 'Tab reaches the expanded navigation destinations');
    await key('\n');
    assert.equal(app.active_view, 1, 'Enter activates a navigation destination');
    assert.equal(app.test_navigation_expanded, false);
    await click(app.test_menu_position, 20, 20);
    await key('\x1b');
    assert.equal(app.test_navigation_expanded, false);
    await click(app.test_menu_position, 20, 20);
    visible(app.test_nav_artists_position, 100, 42);
    await click(app.test_nav_artists_position, 80, 21);
    assert.equal(app.active_view, 1);
    assert.equal(app.test_navigation_expanded, false);
    app.is_busy = true;
    app.busy_label = 'Scanning music folders and reconciling discovered releases';
    await click(app.test_menu_position, 20, 20);
    assert(app.test_nav_scroll_maximum > 0, 'short busy navigation must scroll');
    app.window.dispatchEvent({ type: 'pointer-scrolled', position: { x: 170, y: 320 }, deltaX: 0, deltaY: -1000 });
    await pause(120);
    visible(app.test_nav_cancel_position, 100, 32);
    await click(app.test_nav_cancel_position, 60, 16);
    assert.equal(events.cancel, 1);
    assert.equal(app.test_nav_cancel_focused, true);
    app.is_busy = false;
    await pause(50);
    assert.equal(app.test_nav_cancel_focused, false, 'focus state is cleared when Cancel disappears');
    await key('\x1b');
    assert.equal(app.test_navigation_expanded, false, 'Escape works after the task finishes');
    console.log('PASS compact navigation, Escape, and short-window task cancellation');

    // Player keeps all actions on screen at narrow widths and on short pages.
    app.active_view = 0;
    app.preview_active = true; app.preview_playing = true;
    app.preview_track_title = 'A long example song'; app.preview_artist_name = 'An artist';
    await pause(280);
    assert(app.test_player_height >= 110);
    closeEnough(app.test_player_position.y + app.test_player_height, app.test_height, 'player bottom');
    visible(app.test_play_position, 40, 40);
    visible(app.test_stop_position, 32, 32);
    visible(app.test_volume_position, 32, 32);
    assert(app.test_seek_width >= 40);
    assert(app.test_radar_chip_maximum > 0, 'narrow radar filters scroll horizontally');
    visible(app.test_radar_chip_bar_position, app.test_radar_chip_bar_width, 10);
    await click(app.test_radar_chip_bar_position, app.test_radar_chip_bar_width - 2, 5);
    assert(app.test_radar_chip_scroll > 0);
    visible(app.test_singles_position, app.test_singles_width, 32);
    await click(app.test_singles_position, app.test_singles_width / 2, 16);
    assert.equal(app.feed_type_index, 3, 'rightmost radar filter remains usable');
    await click(app.test_play_position, 20, 20);
    assert.equal(events.pause, 1);
    await click(app.test_seek_position, app.test_seek_width * 0.7, 14);
    assert(events.seek.at(-1) > 0.6 && events.seek.at(-1) < 0.8);
    await click(app.test_volume_position, 16, 16);
    assert.equal(events.volume.at(-1), 0);
    assert(app.test_radar_page_maximum > 0, 'short radar must scroll to its grid');
    app.test_scroll_radar_page(app.test_radar_page_maximum);
    await pause(120);
    assert(app.test_radar_page_scroll > 0);
    const card = app.test_grid_position;
    await click(card, 20 + app.test_card_width / 2, 35);
    assert.equal(events.selections.at(-1), 1, 'grid remains clickable after scrolling the short page');
    await key('\x1b');
    await click(app.test_stop_position, 16, 16);
    assert.equal(events.stopped, 1);
    console.log('PASS responsive preview transport and short radar scrolling');

    // A large result set cannot push the player, filters, or remaining artists
    // permanently out of a short window; both page and results can scroll.
    app.active_view = 1; app.preview_active = true; app.show_add_artist_bar = true;
    app.artist_search_candidates = candidates; app.manual_artist_query = 'Example';
    await pause(280);
    closeEnough(app.test_player_position.y + app.test_player_height, app.test_height, 'player remains visible with many candidates');
    assert(app.test_candidates_maximum > 0);
    assert(app.test_artists_page_maximum > 0);
    app.test_scroll_artists_page(Math.max(0, app.test_candidates_position.y - 120));
    await pause(100);
    let candidatePosition = app.test_candidates_position;
    let wheelPoint = { x: candidatePosition.x + 30, y: candidatePosition.y + 24 };
    app.window.dispatchEvent({ type: 'pointer-scrolled', position: wheelPoint, deltaX: 0, deltaY: -1000 });
    await pause(100);
    assert(app.test_candidates_scroll > 0);
    app.window.dispatchEvent({ type: 'pointer-scrolled', position: wheelPoint, deltaX: 0, deltaY: 1000 });
    await pause(100);
    candidatePosition = app.test_candidates_position;
    await click(candidatePosition, app.test_candidates_width - 45, 24);
    assert.deepEqual(events.candidates, [0]);
    assert.equal(app.show_add_artist_bar, false);
    await key('/');
    await key('z');
    assert.equal(app.artist_search_text, 'z', 'shortcuts still work after tracking a candidate');
    app.artist_search_text = '';
    app.test_scroll_artists_page(app.test_artists_page_maximum);
    await pause(100);
    assert(app.test_artist_filter_position.y >= 0 && app.test_artist_filter_position.y < app.test_player_position.y,
        'artist filters can be reached by scrolling');
    if (app.test_artist_chip_maximum > 0) {
        visible(app.test_artist_chip_bar_position, app.test_artist_chip_bar_width, 10);
        await click(app.test_artist_chip_bar_position, app.test_artist_chip_bar_width - 2, 5);
        assert(app.test_artist_chip_offset > 0);
    }
    visible(app.test_manual_chip_position, app.test_manual_chip_width, 32);
    await click(app.test_manual_chip_position, app.test_manual_chip_width / 2, 16);
    assert.equal(app.artist_filter_index, 4, 'rightmost artist filter remains usable');
    app.show_add_artist_bar = false; app.artist_search_candidates = [];
    app.preview_active = false;
    console.log('PASS scrollable artist search results and short-page filters');

    // Settings controls wrap and retain their callbacks at the minimum size.
    app.active_view = 2; app.new_folder_path = '/music/example';
    await pause(280);
    closeEnough(app.test_settings_visible_width, app.test_settings_content_width, 'settings should fit at minimum width');
    assert(app.test_settings_column_x + app.test_settings_column_width <= app.test_settings_content_width + 1);
    assert(app.test_scale_rows > 1, 'scale choices wrap at narrow widths');
    await revealSettings(app.test_scale_position);
    await gridClick(app.test_scale_position, app.test_scale_cell, app.test_scale_columns, 6);
    assert.equal(events.scale.at(-1), 200);
    assert.equal(app.pref_ui_scale_pct, 200);
    await revealSettings(app.test_swatch_position);
    await gridClick(app.test_swatch_position, 110, app.test_swatch_columns, 2, 44);
    assert.equal(app.pref_accent_preset, 2);
    await revealSettings(app.test_density_position);
    await gridClick(app.test_density_position, app.test_density_cell, app.test_density_columns, 2);
    assert.equal(app.pref_grid_density, 2);
    await revealSettings(app.test_add_path_position);
    await click(app.test_add_path_position, 45, 18);
    await click(app.test_browse_position, 45, 18);
    assert.deepEqual(events.added, ['/music/example']);
    assert.equal(events.browse, 1);
    await revealSettings(app.test_discovery_position);
    await gridClick(app.test_discovery_position, app.test_discovery_cell, app.test_discovery_columns, 2);
    assert.equal(app.pref_discovery_mode, 2);
    assert.equal(events.scanPreferences, 1);
    assert(events.preferences >= 2);
    // Scale changes can bypass native window minimums. At extreme widths the
    // settings page pans instead of squeezing or losing the right side.
    app.window.dispatchEvent({ type: 'resized', size: { width: 350, height: 400 } });
    await pause(120);
    assert(app.test_settings_content_width > app.test_settings_visible_width);
    visible(app.test_settings_bar_position, app.test_settings_bar_width, 10);
    await click(app.test_settings_bar_position, app.test_settings_bar_width - 2, 5);
    assert(app.test_settings_content_x > 0, 'settings scrollbar pans the page');
    console.log('PASS wrapped settings, actions, and extreme-width panning');

    // Both inspector modes remain usable, including keyboard text and Save.
    await resize(685, 514);
    app.active_view = 1; app.selected_artist = artists[0]; app.selected_artist_id = 1; app.inspector_mode = 2;
    app.edit_artist_mbid = ''; app.edit_artist_deezer_id = '42';
    await pause(220);
    closeEnough(app.test_drawer_x + app.test_drawer_width, app.test_width, 'artist inspector right edge');
    visible(app.test_artist_close);
    app.test_scroll_artist_inspector(Math.max(0, app.test_mbid_position.y - 110));
    await pause(120);
    visible(app.test_mbid_position, 120, 50);
    await click(app.test_mbid_position, 40, 35);
    await key('x');
    assert.equal(app.edit_artist_mbid, 'x');
    visible(app.test_mbid_clear_position, 24, 24);
    await click(app.test_mbid_clear_position, 12, 12);
    assert.equal(app.edit_artist_mbid, '');
    await key('y');
    assert.equal(app.edit_artist_mbid, 'y', 'typing continues after clearing an input');
    app.test_scroll_artist_inspector(Math.max(0, app.test_artist_inspector_scroll + app.test_save_position.y - 150));
    await pause(100);
    visible(app.test_save_position, 140, 32);
    await click(app.test_save_position, 70, 16);
    assert.deepEqual(events.saved, [[1, 'y', '42']]);
    await key('\x1b');
    assert.equal(app.inspector_mode, 0);
    console.log('PASS artist inspector, text entry, save, and Escape');

    // A compositor can supply less than the nominal minimum after scaling.
    // Both inspector modes and the overflowing badge row must really pan via
    // their visible scrollbars, not merely report a larger content width.
    await resize(480, 700);
    app.window.dispatchEvent({ type: 'resized', size: { width: 350, height: 700 } });
    app.active_view = 0;
    app.selected_release = ui.ReleaseCardData({
        id: 1, artist_id: 1, title: 'Example Release', artist_name: 'Example Artist',
        release_type: 'ALBUM', monogram: 'E', unseen: true, upcoming: true,
        owned_locally: true, explicit: true, provider_label: 'Deezer + MusicBrainz',
    });
    app.selected_release_id = 1; app.inspector_mode = 1;
    await pause(240);
    closeEnough(app.test_drawer_width, 350, 'inspector caps its width to the window');
    closeEnough(app.test_drawer_x, 0, 'extreme-width inspector begins at left edge');
    visible(app.test_release_close);
    assert(app.test_release_horizontal_maximum > 0);
    assert(app.test_badge_maximum > 0, 'all optional badges overflow into a scrollable row');
    visible(app.test_badge_bar_position, app.test_badge_bar_width, 10);
    await click(app.test_badge_bar_position, app.test_badge_bar_width - 8, 5);
    assert(app.test_badge_offset > 0, 'badge scrollbar exposes the last badge');
    visible(app.test_release_bar_position, app.test_release_bar_width, 10);
    await click(app.test_release_bar_position, app.test_release_bar_width - 2, 5);
    assert(app.test_release_horizontal_offset > 0, 'release details can be panned');
    await key('\x1b');
    app.active_view = 1; app.selected_artist = artists[0]; app.selected_artist_id = 1; app.inspector_mode = 2;
    await pause(240);
    assert(app.test_artist_horizontal_maximum > 0);
    visible(app.test_artist_close);
    visible(app.test_artist_bar_position, app.test_artist_bar_width, 10);
    await click(app.test_artist_bar_position, app.test_artist_bar_width - 2, 5);
    assert(app.test_artist_horizontal_offset > 0, 'artist details can be panned');
    await key('\x1b');
    assert.equal(app.inspector_mode, 0);
    console.log('PASS extreme-width inspector and independently scrollable badges');

    // Long provider badges used to squeeze the candidate name and clip Track.
    // A single result must also get a tall enough list viewport for its row.
    app.window.dispatchEvent({ type: 'resized', size: { width: 350, height: 400 } });
    app.active_view = 1; app.show_add_artist_bar = true; app.manual_artist_query = 'Example';
    app.artist_search_candidates = [ui.SearchCandidateRowData({
        index: 0, name: 'An Example Candidate', subtitle: 'Example result', source_badge: 'MUSICBRAINZ',
    })];
    await pause(200);
    app.test_scroll_artists_page(Math.min(app.test_artists_page_maximum,
        Math.max(0, app.test_artists_page_scroll + app.test_first_row_position.y - 80)));
    await pause(120);
    assert(app.test_first_row_height > 48, 'narrow candidate reflows');
    assert(app.test_first_row_width > 150, 'candidate name keeps usable room');
    assert(app.test_first_track_position.x >= app.test_first_row_position.x);
    assert(app.test_first_track_position.x + app.test_first_track_width
        <= app.test_first_row_position.x + app.test_first_row_width + 1, 'Track stays inside its row');
    assert(app.test_first_track_position.y + 32 <= app.test_candidates_position.y + app.test_candidates_height + 1,
        'single-result list exposes the complete Track action');
    visible(app.test_first_track_position, app.test_first_track_width, 32);
    await click(app.test_first_track_position, app.test_first_track_width / 2, 16);
    assert.deepEqual(events.candidates, [0, 0]);
    app.show_add_artist_bar = false; app.artist_search_candidates = [];
    console.log('PASS extreme-width single artist search result');

    app.active_view = 0; app.preview_active = true;
    await pause(280);
    closeEnough(app.test_player_height, 168, 'extra-narrow player uses three rows');
    visible(app.test_play_position, 40, 40);
    visible(app.test_stop_position, 32, 32);
    visible(app.test_volume_position, 32, 32);
    await click(app.test_stop_position, 16, 16);
    assert.equal(events.stopped, 2);
    console.log('PASS extra-narrow player controls');

    // Empty radar content must not grow wider than the viewport when both
    // actions are shown. They stack and remain clickable at extreme scale.
    app.releases = []; app.feed_status_index = 1;
    app.library_folders = [ui.FolderRowData({ id: 1, path: '/music/example' })];
    await pause(180);
    app.test_scroll_radar_page(app.test_radar_page_maximum);
    await pause(120);
    assert(app.test_empty_card_width > 180 && app.test_empty_card_width < app.test_center_width,
        'empty card fits the center viewport');
    for (const [pos, width] of [[app.test_empty_check_position, app.test_empty_check_width],
        [app.test_empty_reset_position, app.test_empty_reset_width]]) {
        assert(pos.x >= app.test_empty_card_position.x);
        assert(pos.x + width <= app.test_empty_card_position.x + app.test_empty_card_width + 1,
            'empty-state action fits inside the card');
        visible(pos, width, 36);
    }
    assert(app.test_empty_reset_position.y >= app.test_empty_check_position.y + 36,
        'empty-state actions stack at a narrow width');
    await click(app.test_empty_check_position, app.test_empty_check_width / 2, 18);
    assert.equal(events.checks, 1);
    await click(app.test_empty_reset_position, app.test_empty_reset_width / 2, 18);
    assert.equal(app.feed_status_index, 0);
    assert(events.feedFilters > 0);
    app.library_folders = [];
    await pause(180);
    app.test_scroll_radar_page(app.test_radar_page_maximum);
    await pause(100);
    visible(app.test_empty_add_position, app.test_empty_add_width, 36);
    await click(app.test_empty_add_position, app.test_empty_add_width / 2, 18);
    assert.equal(app.active_view, 2, 'empty library action opens settings');
    console.log('PASS narrow radar empty states and actions');

    console.log('All responsive UI regression checks passed.');
    // The native Node binding may tear down an active renderer after Node's
    // event loop; exit explicitly after the checks. The exit hook cleans up.
    process.exit(0);
})().catch(error => {
    console.error(error);
    process.exit(1);
});

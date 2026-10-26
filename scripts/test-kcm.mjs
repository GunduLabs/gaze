// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

// Run with: node scripts/test-kcm.mjs
// Executes Configure's actual QML JavaScript with a simulated KQuickConfigModule.
// Back navigation may only change currentIndex, leaving depth unchanged. This
// does not replace testing the rendered KCM in System Settings and kcmshell6.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import vm from 'node:vm';

const source = await readFile(new URL('../integrations/kde/kcm/ui/main.qml', import.meta.url), 'utf8');
const action = source.match(/Kirigami.Action \{([\s\S]*?)\n        \}/)?.[1];
assert.ok(action?.includes('text: i18n("Configure")'), 'find the Configure action');
const handler = action.match(/onTriggered:\s*(\{[\s\S]*\}|[^\n]+)/)?.[1];
assert.ok(handler, 'find the Configure handler');
const enabled = action.match(/^\s*enabled:\s*(.+)$/m)?.[1] ?? 'true';

// KQuickConfigModule::depth is 1 + subPages.count(), not currentIndex + 1.
// Its push() appends a subpage and increments currentIndex; navigating Back in
// KCModuleQml syncs currentIndex from the PageRow without calling kcm.pop().
// See KDE/kcmutils src/quick/kquickconfigmodule.cpp and src/kcmoduleqml.cpp.
function module() {
    return {
        depth: 1,
        currentIndex: 0,
        pushed: [],
        push(page) {
            this.pushed.push(page);
            this.depth++;
            this.currentIndex++;
        },
    };
}

function configure(kcm) {
    const context = vm.createContext({kcm});
    assert.equal(vm.runInContext(enabled, context), true, 'Configure must remain enabled');
    vm.runInContext(handler, context);
    assert.equal(kcm.currentIndex, 1, 'Settings must become current');
    assert.equal(kcm.depth, 2, 'only one Settings subpage may exist');
}

test('Configure opens Settings the first time', () => {
    const kcm = module();
    configure(kcm);
    assert.deepEqual(kcm.pushed, ['SettingsPage.qml']);
});

test('Configure reopens a retained Settings page after Back', () => {
    const kcm = module();
    configure(kcm);
    kcm.currentIndex = 0;
    configure(kcm);
    assert.deepEqual(kcm.pushed, ['SettingsPage.qml'], 'reuse the retained page');
});

test('repeated Back/Configure cycles do not accumulate Settings pages', () => {
    const kcm = module();
    for (let i = 0; i < 20; i++) {
        configure(kcm);
        kcm.currentIndex = 0;
    }
    assert.equal(kcm.pushed.length, 1);
});

test('Configure recreates Settings if the host popped the subpage', () => {
    const kcm = module();
    configure(kcm);
    kcm.depth = 1;
    kcm.currentIndex = 0;
    configure(kcm);
    assert.deepEqual(kcm.pushed, ['SettingsPage.qml', 'SettingsPage.qml']);
});

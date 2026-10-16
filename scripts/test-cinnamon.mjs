// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

// Run with: node scripts/test-cinnamon.mjs [upstream-source-directory]
// Widgets, signals and D-Bus are simulated; still test in a real Cinnamon session.
import assert from 'node:assert/strict';
import {readFile, writeFile, mkdir, mkdtemp, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {dirname, join} from 'node:path';
import {fileURLToPath} from 'node:url';
import vm from 'node:vm';
import {test, after} from 'node:test';

const repo = fileURLToPath(new URL('../', import.meta.url));
const POLKIT = 'js/ui/polkitAuthenticationAgent.js';
const UNLOCK = 'js/ui/screensaver/unlockDialog.js';
const versions = ['6.2.10', '6.4.14', '6.6.9', '6.7.9-unstable'];
const unlockVersions = new Set(['6.7.9-unstable']);
const upstream = process.argv[2] ?? await mkdtemp(join(tmpdir(), 'gaze-cinnamon-'));
if (!process.argv[2]) {
    after(() => rm(upstream, {recursive: true, force: true}));
    const downloads = await Promise.allSettled(versions.flatMap(version =>
        [POLKIT, ...(unlockVersions.has(version) ? [UNLOCK] : [])].map(async file => {
            const url = `https://raw.githubusercontent.com/linuxmint/cinnamon/${version}/${file}`;
            const response = await fetch(url, {signal: AbortSignal.timeout(30_000)});
            assert.equal(response.status, 200, url);
            const destination = join(upstream, version, file);
            await mkdir(dirname(destination), {recursive: true});
            await writeFile(destination, await response.text());
        })));
    for (const result of downloads)
        if (result.status === 'rejected') throw result.reason;
}

function load(source, exports, globals) {
    const factory = new vm.Script(`(function() {\n${source}\nreturn {${exports}};\n})`)
        .runInNewContext({console, ...globals});
    return factory();
}

class Emitter {
    connect(name, callback) {
        this.handlers ??= new Map();
        this.nextHandlerId = (this.nextHandlerId ?? 0) + 1;
        this.handlers.set(this.nextHandlerId, [name, callback]);
        return this.nextHandlerId;
    }
    disconnect(id) {
        assert.ok(this.handlers?.delete(id), `disconnect of unknown handler ${id}`);
    }
    emit(name, ...args) {
        let result;
        for (const [signal, callback] of [...(this.handlers?.values() ?? [])])
            if (signal === name) result = callback(this, ...args);
        return result;
    }
}

class ClutterText extends Emitter {
    constructor() {
        super();
        this.editable = true;
    }
    set_line_alignment(alignment) { this.alignment = alignment; }
    set_password_char(character) { this.passwordChar = character; }
    insert_unichar() {}
}

class Widget extends Emitter {
    constructor(params = {}) {
        super();
        this._init(params);
    }
    _init(params = {}) {
        this.children = [];
        this.parent = null;
        this.visible = true;
        this.reactive = true;
        this.text = '';
        this.clutter_text = new ClutterText();
        if (typeof params === 'object') Object.assign(this, params);
    }
    add_child(child) {
        child.parent?.remove_child(child);
        this.children.push(child);
        child.parent = this;
    }
    add(child) { this.add_child(child); }
    remove_child(child) {
        this.children = this.children.filter(item => item !== child);
        child.parent = null;
    }
    insert_child_above(child, sibling) {
        assert.equal(sibling?.parent, this, 'insert_child_above requires a direct sibling');
        this.add_child(child);
    }
    get_parent() { return this.parent; }
    set_child(child) { this.add_child(child); }
    show() { this.visible = true; }
    hide() { this.visible = false; }
    set_text(text) { this.text = text; }
    get_text() { return this.text; }
    set_label(label) { this.label = label; }
    grab_key_focus() { this.focused = true; }
    start_busy() { this.busy = true; }
    end_busy() { this.busy = false; }
    set_primary_icon() {}
    bind_property() {}
    play() { this.playing = true; }
    stop() { this.playing = false; }
    destroy() {
        this.parent?.remove_child(this);
        this.destroyed = true;
    }
}

const shown = widget => {
    for (let item = widget; item; item = item.parent)
        if (!item.visible) return false;
    return Boolean(widget);
};

const CONFIRM_LABEL = 'Face verified. Press Enter or click Authenticate to confirm.';
const metadata = JSON.parse(await readFile(join(repo, 'integrations/cinnamon/metadata.json'), 'utf8'));

async function environment(version, options = {}) {
    const errors = [];
    const calls = [];
    const sessions = [];
    const timers = new Map();
    let nextTimer = 0;
    const settings = {
        values: {
            'enable-face-authentication': options.enabled ?? true,
            'face-retry-mode': options.retryMode ?? 'fixed',
            'max-face-tries': options.maxTries ?? 3,
        },
        finalized: false,
    };

    const proxy = new Emitter();
    proxy.g_name_owner = ':1.42';
    for (const method of ['RegisterExtension', 'AddPamInternal', 'RemovePamInternal'])
        proxy[`${method}Remote`] = (...args) => calls.push([method, ...args]);
    class DBusProxy {
        constructor(_bus, _name, _path, callback) {
            queueMicrotask(() => callback(proxy, null));
            return proxy;
        }
    }

    class PolkitSession extends Emitter {
        constructor(params = {}) {
            super();
            Object.assign(this, params);
            this.responses = [];
            this.cancelled = false;
            this.initiated = false;
            sessions.push(this);
        }
        initiate() { this.initiated = true; }
        response(answer) { this.responses.push(answer); }
        cancel() { this.cancelled = true; }
    }

    const GLib = {
        PRIORITY_DEFAULT: 0, SOURCE_REMOVE: false,
        get_user_name: () => 'alice', getenv: () => null,
        timeout_add(_priority, _interval, callback) {
            timers.set(++nextTimer, callback);
            return nextTimer;
        },
        timeout_add_seconds(_priority, _interval, callback) {
            timers.set(++nextTimer, callback);
            return nextTimer;
        },
        source_remove(id) { assert.ok(timers.delete(id), `removed unknown source ${id}`); },
        Source: {set_name_by_id() {}},
    };
    const St = Object.fromEntries(['BoxLayout', 'Label', 'Button', 'Bin', 'Icon',
        'Entry', 'PasswordEntry'].map(name => [name, Widget]));
    Object.assign(St, {IconType: {SYMBOLIC: 1}, Align: {}, Side: {}});
    const Clutter = {
        ActorAlign: {FILL: 0, START: 1, CENTER: 2}, Orientation: {},
        KEY_Return: 0xff0d, KEY_KP_Enter: 0xff8d, KEY_ISO_Enter: 0xfe34,
        KEY_Escape: 0xff1b, Escape: 0xff1b,
        EVENT_STOP: true, EVENT_PROPAGATE: false,
    };
    const global = {
        logError: (...args) => errors.push(args.map(String).join(' ')),
        logWarning: (...args) => errors.push(args.map(String).join(' ')),
        log() {},
        get_current_time: () => 0,
        settings: {get_boolean: () => false},
        stage: {set_key_focus(actor) { actor.focused = true; }},
    };
    const namespace = known => new Proxy(known, {get: (target, key) => key in target ? target[key] : {}});
    const ui = {
        modalDialog: {ModalDialog: Widget, State: {OPENED: 1}},
        popupDialog: {PopupDialog: Widget, State: {OPENED: 1}},
        animation: {Spinner: Widget},
        settings: {
            ExtensionSettings: class {
                getValue(key) { return settings.values[key]; }
                finalize() { settings.finalized = true; }
            },
        },
    };
    const imports = namespace({
        gi: namespace({
            Gio: {DBusProxy: {makeProxyWrapper: () => DBusProxy}, DBus: {system: {}}},
            GLib, St, Clutter,
            Pango: {EllipsizeMode: {NONE: 0}, Alignment: {CENTER: 1}},
            GObject: {registerClass: (...args) => args.at(-1), BindingFlags: {}, TYPE_BOOLEAN: {}},
            Polkit: {UnixUser: {new_for_name: name => ({name})}},
            PolkitAgent: {Session: PolkitSession},
        }),
        ui: namespace(ui),
        misc: namespace({util: {wiggle() {}}}),
        signals: {
            addSignalMethods(proto) {
                for (const name of ['connect', 'disconnect', 'emit'])
                    proto[name] = Emitter.prototype[name];
            },
        },
        lang: {bind: (object, method) => method.bind(object)},
        mainloop: {timeout_add: (interval, callback) => GLib.timeout_add(0, interval, callback)},
    });
    const globals = {imports, global, _: text => text, log() {}};

    let Polkit = null;
    let Unlock = null;
    const patched = [];
    if (version !== 'none') {
        Polkit = load(await readFile(join(upstream, version, POLKIT), 'utf8'),
            'AuthenticationDialog, AuthenticationAgent', globals);
        Polkit.AuthenticationAgent.prototype._onInitiate = function () {
            this._currentDialog = this.testDialog;
            startSession(this._currentDialog, PolkitSession);
        };
        ui.polkitAuthenticationAgent = Polkit;
        patched.push(Polkit.AuthenticationAgent.prototype, Polkit.AuthenticationDialog.prototype);
    }
    if (unlockVersions.has(version)) {
        Unlock = load(await readFile(join(upstream, version, UNLOCK), 'utf8'), 'UnlockDialog', globals);
        ui.screensaver = {unlockDialog: Unlock};
        patched.push(Unlock.UnlockDialog.prototype);
    }
    const before = patched.map(snapshot);

    const extension = load(await readFile(join(repo, 'integrations/cinnamon/extension.js'), 'utf8'),
        'init, enable, disable, instance: () => extensionInstance', globals);
    extension.init(metadata);
    extension.enable();
    await flush();

    function restored() {
        patched.forEach((proto, i) => assert.deepEqual(snapshot(proto), before[i]));
    }
    return {version, extension, proxy, calls, errors, sessions, timers, settings,
        Polkit, Unlock, Clutter, restored};
}

function snapshot(proto) {
    return Object.fromEntries(Object.getOwnPropertyNames(proto).map(name => [name, proto[name]]));
}

async function flush() {
    for (let i = 0; i < 8; i++) await Promise.resolve();
}

function noErrors(env) {
    assert.equal(env.errors.length, 0, env.errors.join('\n'));
}

function startSession(dialog, PolkitSession) {
    if (typeof dialog.performAuthentication === 'function') {
        dialog.performAuthentication();
        return;
    }
    dialog._session = new PolkitSession();
    for (const [signal, method] of [['completed', '_onSessionCompleted'], ['request', '_onSessionRequest'],
        ['show-error', '_onSessionShowError'], ['show-info', '_onSessionShowInfo']])
        dialog._session.connect(signal, dialog[method].bind(dialog));
    dialog.startAuthentication();
}

function polkitDialog(env) {
    const dialog = Object.create(env.Polkit.AuthenticationDialog.prototype);
    const legacy = typeof dialog.performAuthentication !== 'function';
    const root = new Widget();
    const passwordBox = new Widget({visible: !legacy});
    const passwordEntry = new Widget({visible: legacy, hint_text: ''});
    passwordBox.add_child(passwordEntry);
    const warningBox = new Widget();
    const errorLabel = new Widget({visible: false});
    const infoLabel = new Widget({visible: false});
    const nullLabel = new Widget();
    for (const label of [errorLabel, infoLabel, nullLabel]) warningBox.add_child(label);
    root.add_child(passwordBox);
    root.add_child(warningBox);
    Object.assign(dialog, {
        _passwordBox: legacy ? passwordBox : undefined,
        _passwordLabel: new Widget(),
        _passwordEntry: passwordEntry,
        _errorMessageLabel: errorLabel,
        _infoMessageLabel: infoLabel,
        _nullMessageLabel: nullLabel,
        _okButton: new Widget({reactive: false}),
        _cancelButton: new Widget(),
        _cookie: 'cookie',
        _identityToAuth: {name: 'alice'},
        _completed: false,
        _wasDismissed: false,
        _doneEmitted: false,
        _sessionRequestTimeoutId: 0,
        _sessionCompletedId: 0,
        state: 1,
        open() {
            this.opened = true;
            return true;
        },
    });
    return dialog;
}

function beginPolkit(env) {
    const dialog = polkitDialog(env);
    const agent = Object.create(env.Polkit.AuthenticationAgent.prototype);
    agent.testDialog = dialog;
    agent._onInitiate({}, 'org.example.action', 'Authenticate', 'dialog-password', 'cookie', ['alice']);
    assert.equal(dialog._session.initiated, true);
    return dialog;
}

function unlockDialog(env) {
    const dialog = Object.create(env.Unlock.UnlockDialog.prototype);
    const dialogBox = new Widget();
    const contentLayout = new Widget();
    const passwordBox = new Widget();
    dialogBox.add_child(contentLayout);
    contentLayout.add_child(passwordBox);
    const passwordEntry = new Widget({visible: false, hint_text: 'Password'});
    const capsLockWarning = new Widget({visible: false});
    const infoLabel = new Widget();
    const messageLabel = new Widget();
    for (const child of [passwordEntry, capsLockWarning, infoLabel, messageLabel])
        passwordBox.add_child(child);
    Object.assign(dialog, {
        children: [],
        visible: true,
        _dialogBox: dialogBox,
        _contentLayout: contentLayout,
        _passwordEntry: passwordEntry,
        _capsLockWarning: capsLockWarning,
        _infoLabel: infoLabel,
        _messageLabel: messageLabel,
        _authClient: {
            initialized: true, sent: [], cancels: 0, initializations: 0,
            sendPassword(password) { this.sent.push(password); },
            cancel() {
                this.cancels++;
                this.initialized = false;
            },
            initialize() {
                this.initializations++;
                this.initialized = true;
                return true;
            },
        },
        _screenShield: {
            unlocked: false, hidden: false,
            unlock() { this.unlocked = true; },
            hideUnlockDialog() { this.hidden = true; },
        },
    });
    return dialog;
}

function pressKey(dialog, symbol) {
    return dialog._onKeyPress(dialog, {get_key_symbol: () => symbol});
}

const registration = [['RegisterExtension', true], ['AddPamInternal', 'polkit-1'], ['AddPamInternal', 'cinnamon']];

for (const version of ['none', ...versions]) {
    const label = version === 'none' ? 'without upstream hooks' : version;

    test(`${label}: registers with gazed for the Cinnamon PAM services`, async () => {
        const env = await environment(version);
        try {
            assert.deepEqual(env.calls, registration);
            env.proxy.emit('notify::g-name-owner');
            assert.deepEqual(env.calls, [...registration, ...registration]);
            noErrors(env);
        } finally { env.extension.disable(); }
    });

    test(`${label}: disabling unregisters and restores upstream hooks`, async () => {
        const env = await environment(version);
        let widgets = [];
        if (env.Polkit) {
            const dialog = beginPolkit(env);
            dialog._session.emit('show-info', 'GAZE_MSG_LOOK_CAMERA');
        }
        if (env.Unlock) {
            const dialog = unlockDialog(env);
            dialog._onAuthPrompt(dialog._authClient, 'GAZE_REQUIRE_CONFIRMATION');
            widgets = [dialog._confirmButton, dialog._confirmSpinnerBin];
        }
        env.calls.length = 0;
        env.extension.disable();
        assert.deepEqual(env.calls, [['RemovePamInternal', 'polkit-1'], ['RemovePamInternal', 'cinnamon'],
            ['RegisterExtension', false]]);
        assert.equal(env.proxy.handlers.size, 0);
        assert.equal(env.settings.finalized, true);
        for (const widget of widgets) assert.equal(widget.destroyed, true);
        env.restored();
        noErrors(env);
    });
}

for (const version of versions) {
    test(`${version}: upstream still has every Polkit method Gaze patches`, async () => {
        const source = await readFile(join(upstream, version, POLKIT), 'utf8');
        for (const name of ['_onInitiate', '_onSessionShowInfo', '_onSessionShowError',
            '_onSessionRequest', '_onEntryActivate', '_onAuthenticateButtonPressed'])
            assert.match(source, new RegExp(`\\b${name}\\s*(:\\s*function)?\\s*\\(`), `missing upstream hook: ${name}`);
    });

    test(`${version}: starting a Polkit request re-registers with gazed`, async () => {
        const env = await environment(version);
        try {
            env.calls.length = 0;
            beginPolkit(env);
            assert.deepEqual(env.calls, registration);
            noErrors(env);
        } finally { env.extension.disable(); }
    });

    for (const signal of ['request', 'show-info']) {
        for (const action of ['Enter', 'Authenticate button']) {
            test(`${version}: ${signal} confirmation via ${action} answers PAM`, async () => {
                const env = await environment(version);
                try {
                    const dialog = beginPolkit(env);
                    dialog._session.emit(signal, 'GAZE_REQUIRE_CONFIRMATION', false);
                    assert.equal(dialog._confirmMode, true);
                    assert.equal(shown(dialog._passwordEntry), false);
                    assert.equal(dialog._infoMessageLabel.text, CONFIRM_LABEL);
                    assert.equal(shown(dialog._infoMessageLabel), true);
                    assert.equal(dialog._okButton.reactive, true);
                    if (action === 'Enter') dialog._onEntryActivate();
                    else dialog._onAuthenticateButtonPressed();
                    assert.deepEqual(dialog._session.responses, ['GAZE_CONFIRMED']);
                    assert.equal(dialog._confirmMode, false);
                    noErrors(env);
                } finally { env.extension.disable(); }
            });
        }
    }

    test(`${version}: the legacy confirmation question answers with an empty response`, async () => {
        const env = await environment(version);
        try {
            const dialog = beginPolkit(env);
            dialog._session.emit('request', 'Face Verified. Press Enter to confirm.', false);
            assert.equal(dialog._confirmMode, true);
            dialog._onAuthenticateButtonPressed();
            assert.deepEqual(dialog._session.responses, ['']);
            noErrors(env);
        } finally { env.extension.disable(); }
    });

    test(`${version}: password prompts still accept their own answers`, async () => {
        const env = await environment(version);
        try {
            const dialog = beginPolkit(env);
            dialog._session.emit('request', 'Password:', false);
            assert.equal(dialog._confirmMode ?? false, false);
            assert.equal(shown(dialog._passwordEntry), true, 'password entry is on screen');
            assert.equal(dialog._passwordEntry.reactive, true);
            assert.equal(dialog._passwordEntry.hint_text, 'Password');
            dialog._passwordEntry.set_text('test-password');
            dialog._onEntryActivate();
            assert.deepEqual(dialog._session.responses, ['test-password']);
            noErrors(env);
        } finally { env.extension.disable(); }
    });

    test(`${version}: a password prompt replaces face confirmation`, async () => {
        const env = await environment(version);
        try {
            const dialog = beginPolkit(env);
            dialog._session.emit('request', 'GAZE_REQUIRE_CONFIRMATION', false);
            dialog._session.emit('request', 'Password:', false);
            assert.equal(dialog._confirmMode, false);
            assert.equal(shown(dialog._infoMessageLabel) && dialog._infoMessageLabel.text === CONFIRM_LABEL, false);
            assert.equal(shown(dialog._passwordEntry), true, 'password entry is on screen');
            assert.equal(dialog._passwordEntry.reactive, true);
            dialog._passwordEntry.set_text('test-password');
            dialog._onEntryActivate();
            assert.deepEqual(dialog._session.responses, ['test-password']);
            noErrors(env);
        } finally { env.extension.disable(); }
    });

    test(`${version}: camera hints show beside the password entry`, async () => {
        const env = await environment(version);
        try {
            const dialog = beginPolkit(env);
            dialog._session.emit('show-info', 'GAZE_MSG_LOOK_CAMERA');
            const hint = dialog._faceHintLabel;
            assert.equal(hint.parent, dialog._errorMessageLabel.parent);
            assert.equal(hint.visible, true);
            assert.equal(hint.text, '(or look at the camera)');
            assert.equal(dialog._infoMessageLabel.visible, false);
            assert.equal(dialog._errorMessageLabel.visible, false);
            assert.equal(dialog._passwordEntry.reactive, true);
            dialog._session.emit('show-info', 'Hold still...');
            assert.equal(hint.text, 'Hold still...');
            dialog._session.emit('show-info', 'GAZE_MSG_FACE_VERIFIED');
            assert.equal(hint.visible, false);
            noErrors(env);
        } finally { env.extension.disable(); }
    });

    for (const [signal, message, expected] of [
        ['show-info', 'GAZE_MSG_FACE_NOT_RECOGNIZED', 'Face not recognized. Please enter your password.'],
        ['show-info', 'GAZE_MSG_FACE_TOO_DARK', 'Too dark for face authentication. Please enter your password.'],
        ['show-info', 'GAZE_MSG_SOMETHING_NEW', 'Face authentication failed. Please enter your password.'],
        ['show-error', 'Sorry, that didn’t work. Please try again.', 'Sorry, face authentication did not work. Please try again.'],
        ['show-error', 'Authentication failure', 'Authentication failure'],
    ]) {
        test(`${version}: ${signal} ${message} shows a readable error`, async () => {
            const env = await environment(version);
            try {
                const dialog = beginPolkit(env);
                dialog._session.emit('show-info', 'GAZE_MSG_LOOK_CAMERA');
                dialog._session.emit(signal, message);
                assert.equal(dialog._errorMessageLabel.text, expected);
                assert.equal(dialog._errorMessageLabel.visible, true);
                assert.equal(dialog._faceHintLabel.visible, false);
                noErrors(env);
            } finally { env.extension.disable(); }
        });
    }

    test(`${version}: unrelated PAM info reaches the upstream handler`, async () => {
        const env = await environment(version);
        try {
            const dialog = beginPolkit(env);
            dialog._session.emit('show-info', 'Swipe your finger across the reader');
            assert.equal(dialog._infoMessageLabel.text, 'Swipe your finger across the reader');
            assert.equal(dialog._infoMessageLabel.visible, true);
            noErrors(env);
        } finally { env.extension.disable(); }
    });

    if (version !== '6.2.10') {
        test(`${version}: a failed attempt retries with the hooks still in place`, async () => {
            const env = await environment(version);
            try {
                const dialog = beginPolkit(env);
                const first = dialog._session;
                first.emit('completed', false);
                assert.notEqual(dialog._session, first);
                assert.equal(env.sessions.length, 2);
                assert.equal(env.timers.size, 0, 'delayed reset cancelled');
                assert.equal(shown(dialog._passwordEntry), true);
                assert.equal(dialog._passwordEntry.reactive, true);
                dialog._session.emit('request', 'GAZE_REQUIRE_CONFIRMATION', false);
                assert.equal(dialog._confirmMode, true);
                dialog._onAuthenticateButtonPressed();
                assert.deepEqual(first.responses, []);
                assert.deepEqual(dialog._session.responses, ['GAZE_CONFIRMED']);
                noErrors(env);
            } finally { env.extension.disable(); }
        });
    }
}

for (const version of versions.filter(version => unlockVersions.has(version))) {
    test(`${version}: upstream still has every unlock method Gaze patches`, async () => {
        const source = await readFile(join(upstream, version, UNLOCK), 'utf8');
        for (const name of ['_onAuthInfo', '_onAuthPrompt', '_onAuthError', '_onAuthSuccess',
            '_onAuthFailure', '_onAuthCancel', '_onUnlock', '_onKeyPress', '_onCancel',
            'initializePam', '_setBusy'])
            assert.match(source, new RegExp(`\\b${name}\\s*\\(`), `missing upstream hook: ${name}`);
    });

    for (const signal of ['prompt', 'info']) {
        for (const action of ['Return', 'KP_Enter', 'ISO_Enter', 'click', 'entry']) {
            test(`${version}: unlock ${signal} confirmation via ${action} answers PAM`, async () => {
                const env = await environment(version);
                try {
                    const dialog = unlockDialog(env);
                    const client = dialog._authClient;
                    if (signal === 'prompt') dialog._onAuthPrompt(client, 'GAZE_REQUIRE_CONFIRMATION');
                    else dialog._onAuthInfo(client, 'GAZE_REQUIRE_CONFIRMATION');
                    assert.equal(dialog._confirmMode, true);
                    assert.equal(shown(dialog._passwordEntry), false);
                    const button = dialog._confirmButton;
                    assert.equal(button.parent, dialog._contentLayout);
                    assert.equal(shown(button), true);
                    assert.equal(button.focused, true);
                    if (action === 'click') button.emit('clicked');
                    else if (action === 'entry') dialog._onUnlock();
                    else assert.equal(pressKey(dialog, env.Clutter[`KEY_${action}`]), env.Clutter.EVENT_STOP);
                    assert.deepEqual(client.sent, ['GAZE_CONFIRMED']);
                    assert.equal(dialog._confirmMode, false);
                    assert.equal(button.visible, false);
                    assert.equal(dialog._confirmSpinnerBin.visible, true);
                    assert.equal(dialog._confirmSpinner.playing, true);
                    assert.equal(dialog._passwordEntry.busy, true);
                    noErrors(env);
                } finally { env.extension.disable(); }
            });
        }
    }

    test(`${version}: Escape cancels unlock confirmation without answering PAM`, async () => {
        const env = await environment(version);
        try {
            const dialog = unlockDialog(env);
            dialog._onAuthPrompt(dialog._authClient, 'GAZE_REQUIRE_CONFIRMATION');
            assert.equal(pressKey(dialog, env.Clutter.KEY_Escape), env.Clutter.EVENT_STOP);
            assert.equal(dialog._confirmMode, false);
            assert.equal(dialog._confirmButton.visible, false);
            assert.equal(dialog._authClient.cancels, 1);
            assert.equal(dialog._screenShield.hidden, true);
            assert.deepEqual(dialog._authClient.sent, []);
            noErrors(env);
        } finally { env.extension.disable(); }
    });

    test(`${version}: a password prompt replaces unlock confirmation`, async () => {
        const env = await environment(version);
        try {
            const dialog = unlockDialog(env);
            dialog._onAuthPrompt(dialog._authClient, 'GAZE_REQUIRE_CONFIRMATION');
            dialog._onAuthPrompt(dialog._authClient, 'Password:');
            assert.equal(dialog._confirmMode, false);
            assert.equal(dialog._confirmButton.visible, false);
            assert.equal(shown(dialog._passwordEntry), true);
            assert.equal(dialog._passwordEntry.reactive, true);
            assert.equal(dialog._passwordEntry.focused, true);
            dialog._passwordEntry.set_text('test-password');
            dialog._onUnlock();
            assert.deepEqual(dialog._authClient.sent, ['test-password']);
            noErrors(env);
        } finally { env.extension.disable(); }
    });

    test(`${version}: unlock camera hints use the info label`, async () => {
        const env = await environment(version);
        try {
            const dialog = unlockDialog(env);
            const client = dialog._authClient;
            dialog._passwordEntry.hint_text = 'GAZE_MSG_LOOK_CAMERA';
            dialog._onAuthPrompt(client, 'GAZE_MSG_LOOK_CAMERA');
            assert.equal(dialog._infoLabel.text, '(or look at the camera)');
            assert.equal(dialog._passwordEntry.hint_text, 'Password');
            dialog._onAuthInfo(client, 'Hold still...');
            assert.equal(dialog._infoLabel.text, 'Hold still...');
            dialog._onAuthInfo(client, 'GAZE_MSG_FACE_VERIFIED');
            assert.equal(dialog._infoLabel.text, '');
            noErrors(env);
        } finally { env.extension.disable(); }
    });

    for (const [handler, message, field, expected] of [
        ['_onAuthInfo', 'GAZE_MSG_FACE_NOT_RECOGNIZED', '_infoLabel', 'Face not recognized. Please enter your password.'],
        ['_onAuthInfo', 'GAZE_MSG_SOMETHING_NEW', '_infoLabel', 'Face authentication failed. Please enter your password.'],
        ['_onAuthError', 'Sorry, that didn’t work. Please try again.', '_messageLabel', 'Sorry, face authentication did not work. Please try again.'],
        ['_onAuthError', 'Authentication failure', '_messageLabel', 'Authentication failure'],
    ]) {
        test(`${version}: unlock ${handler} ${message} shows a readable message`, async () => {
            const env = await environment(version);
            try {
                const dialog = unlockDialog(env);
                dialog[handler](dialog._authClient, message);
                assert.equal(dialog[field].text, expected);
                noErrors(env);
            } finally { env.extension.disable(); }
        });
    }

    for (const [retryMode, enabled, maxTries, retries] of [
        ['disabled', true, 3, 0], ['fixed', true, 3, 2], ['fixed', true, 1, 1],
        ['infinite', true, 3, 3], ['fixed', false, 3, 0],
    ]) {
        test(`${version}: ${retryMode} retries (enabled=${enabled}, max=${maxTries}) restart face unlock ${retries} times`, async () => {
            const env = await environment(version, {retryMode, enabled, maxTries});
            try {
                const dialog = unlockDialog(env);
                for (let i = 0; i < 3; i++) {
                    dialog._authClient.initialized = false;
                    dialog._onAuthFailure();
                }
                assert.equal(dialog._authClient.initializations, retries);
                assert.equal(dialog._faceFailCounter, 3);
                assert.equal(dialog._faceAuthFailed === true, retries < 3);
                noErrors(env);
            } finally { env.extension.disable(); }
        });
    }

    test(`${version}: success and cancel reset the face failure count`, async () => {
        const env = await environment(version);
        try {
            const dialog = unlockDialog(env);
            dialog._authClient.initialized = false;
            dialog._onAuthFailure();
            assert.equal(dialog._faceFailCounter, 1);
            dialog._onAuthCancel();
            assert.equal(dialog._faceFailCounter, 0);
            assert.equal(dialog._authClient.initialized, true);
            dialog._onAuthFailure();
            dialog._onAuthPrompt(dialog._authClient, 'GAZE_REQUIRE_CONFIRMATION');
            dialog._onAuthSuccess();
            assert.equal(dialog._faceFailCounter, 0);
            assert.equal(dialog._faceAuthFailed, false);
            assert.equal(dialog._confirmButton.visible, false);
            assert.equal(dialog._screenShield.unlocked, true);
            noErrors(env);
        } finally { env.extension.disable(); }
    });
}

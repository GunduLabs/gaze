// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

// Run with: node scripts/test-gnome.mjs [upstream-source-directory]
// Downloads GNOME Shell 45.0 through 51.0 when no source directory is given.
// Executes upstream authentication classes and Gaze's extension together.
// Native widgets, GObject signals and D-Bus are simulated; this is not a
// replacement for testing login/unlock in an actual GNOME desktop session.
import assert from 'node:assert/strict';
import {readFile, writeFile, mkdir, mkdtemp, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {dirname, join} from 'node:path';
import {fileURLToPath} from 'node:url';
import vm from 'node:vm';
import {test, after} from 'node:test';

const repo = fileURLToPath(new URL('../', import.meta.url));
const versions = ['45.0', '46.0', '47.0', '48.0', '49.0', '50.0', '51.0'];
const upstream = process.argv[2] ?? await mkdtemp(join(tmpdir(), 'gaze-gnome-'));
if (!process.argv[2]) {
    after(() => rm(upstream, {recursive: true, force: true}));
    const common = ['gdm/util.js', 'gdm/authPrompt.js',
        'gdm/batch.js', 'ui/components/polkitAgent.js', 'extensions/extension.js'];
    const downloads = await Promise.allSettled(versions.flatMap(version => {
        const files = version === '51.0'
            ? [...common, 'gdm/userVerifier.js', 'gdm/authServices.js', 'gdm/authServicesLegacy.js']
            : common;
        return files.map(async file => {
            const url = `https://raw.githubusercontent.com/GNOME/gnome-shell/${version}/js/${file}`;
            const response = await fetch(url, {signal: AbortSignal.timeout(30_000)});
            assert.equal(response.status, 200, url);
            const destination = join(upstream, version, 'js', file);
            await mkdir(dirname(destination), {recursive: true});
            await writeFile(destination, await response.text());
        });
    }));
    for (const result of downloads)
        if (result.status === 'rejected') throw result.reason;
}

// Preserve the actual upstream class bodies; replace only their native imports.
function load(source, dependencies, exports, globals = {}) {
    const body = source.replace(/^import\s[\s\S]*?;\s*$/gm, '')
        .replace(/^export \{.*?\};$/gm, '')
        .replace(/export default class /g, 'class ')
        .replace(/\bexport (?=const |class |function )/g, '');
    const factory = new vm.Script(`(function(${Object.keys(dependencies)}) {
        ${body}
        return {${exports}};
    })`).runInNewContext({console, _, ...globals});
    return factory(...Object.values(dependencies));
}

const _ = text => text;
const signalKey = Symbol('GObject.signals');
const Gi = {gobject_prototype_symbol: Symbol('gobject_prototype'), hook_up_vfunc_symbol: Symbol('hook_up_vfunc')};
class Emitter {
    connect(name, callback) {
        this.listeners ??= new Map();
        const callbacks = this.listeners.get(name) ?? [];
        callbacks.push(callback);
        this.listeners.set(name, callbacks);
        return callbacks.length;
    }
    connectObject(...args) {
        for (let i = 0; i < args.length - 1; i += 2)
            this.connect(args[i], args[i + 1]);
    }
    disconnectObject() { this.listeners?.clear(); }
    emit(name, ...args) {
        const declaration = this.constructor[signalKey]?.[name];
        if (declaration)
            assert.equal(args.length, declaration.param_types?.length ?? 0, name);
        this.events ??= [];
        this.events.push([name, ...args]);
        for (const callback of this.listeners?.get(name) ?? [])
            callback(this, ...args);
    }
}

class Widget extends Emitter {
    constructor(params = {}) {
        super();
        this._init(params);
    }
    _init(params = {}) {
        this.children = [];
        this.visible = true;
        this.reactive = true;
        this.clutter_text = new Emitter();
        this.clutterText = {set: values => Object.assign(this.clutterText, values)};
        Object.assign(this, params);
    }
    add_child(child) {
        child.parent?.remove_child(child);
        this.children.push(child);
        child.parent = this;
    }
    remove_child(child) {
        this.children = this.children.filter(item => item !== child);
        child.parent = null;
    }
    insert_child_below(child, sibling) {
        assert.equal(sibling.parent, this, 'insert_child_below requires a direct sibling');
        this.add_child(child);
    }
    replace_child(oldChild, child) {
        assert.equal(oldChild.parent, this);
        this.remove_child(oldChild);
        this.add_child(child);
    }
    get_parent() { return this.parent; }
    set_child(child) { this.add_child(child); }
    get_child() { return this.children[0]; }
    show() { this.visible = true; }
    hide() { this.visible = false; }
    set_text(text) { this.text = text; }
    get_text() { return this.text ?? ''; }
    set(values) { Object.assign(this, values); }
    grab_key_focus() { this.focused = true; }
    add_constraint() {}
    bind_property() {}
    add_style_pseudo_class() {}
    remove_style_class_name() {}
    add_style_class_name() {}
    ease(params) { Object.assign(this, params); params.onComplete?.(); }
    remove_all_transitions() {}
    clear() {}
    get_preferred_height() { return [0, 0]; }
    get_accessible() { return {notify() {}, notify_state_change() {}, emit() {}}; }
    destroy() { this.parent?.remove_child(this); this.destroyed = true; }
    play() { this.playing = true; }
    stop() { this.playing = false; }
    get [Gi.gobject_prototype_symbol]() {
        const prototype = this;
        return {[Gi.hook_up_vfunc_symbol]: (name, method) => { prototype[`vfunc_${name}`] = method; }};
    }
}

async function environment(version, options = {}) {
    const errors = [];
    const answers = [];
    const starts = [];
    const deferred = [];
    const settings = {
        get_boolean: key => key === 'enable-face-authentication'
            ? options.enabled !== false : key === 'enable-password-authentication',
        get_int: key => key === 'max-face-tries' ? 2 : 3,
        get_string: () => options.retryMode ?? 'fixed',
    };
    const proxy = new Emitter();
    const remote = new Emitter();
    remote.get_connection = () => new Emitter();
    remote.call_begin_verification_for_user = async (service, user) => starts.push([service, user]);
    remote.call_begin_verification = async service => starts.push([service, null]);
    remote.call_answer_query = async (service, answer) => answers.push([service, answer]);
    remote.call_cancel_sync = () => {};
    function reply(callback, value) {
        const action = () => callback([value], null);
        if (options.deferProbes) deferred.push(action);
        else queueMicrotask(action);
    }
    proxy.HasEnrolledFacesRemote = (user, callback) => reply(callback, options.enrolled !== false);
    proxy.IsCameraAvailableRemote = callback => reply(callback, options.camera !== false);
    for (const name of ['RegisterExtensionRemote', 'AddPamInternalRemote', 'RemovePamInternalRemote'])
        proxy[name] = () => {};
    class DBusProxy {
        constructor(_bus, _name, _path, callback) {
            if (callback) queueMicrotask(() => callback(proxy, null));
            return proxy;
        }
    }
    const Gio = {
        DBusProxy: {makeProxyWrapper: () => DBusProxy},
        DBusInterfaceInfo: {new_for_xml: () => ({})},
        DBus: {system: {}}, _promisify() {},
        Cancellable: class { cancel() {} },
        Settings: class extends Emitter {
            get_boolean(key) { return settings.get_boolean(key); }
            get_int(key) { return settings.get_int(key); }
        },
    };
    const Gdm = Object.fromEntries(['Client', 'UserVerifierProxy',
        'UserVerifierChoiceListProxy', 'UserVerifierCustomJSONProxy'].map(name => [name, class {}]));
    const GLib = {
        getenv: () => null, source_remove() {},
        timeout_add: () => 1, timeout_add_once: () => 1,
        timeout_add_seconds_once: () => 1,
        Source: {set_name_by_id() {}}, Error,
    };
    const GObject = {
        Object: Emitter, signals: signalKey, TYPE_JSOBJECT: {},
        ParamSpec: {uint() {}}, ParamFlags: {}, BindingFlags: {},
        registerClass(...args) {
            const klass = args.at(-1);
            klass.$gtype = klass;
            return klass;
        },
        signal_lookup: (name, klass) => klass[signalKey]?.[name],
        signal_query: declaration => ({n_params: declaration.param_types?.length ?? 0}),
    };
    const St = Object.fromEntries(['BoxLayout', 'Widget', 'Button', 'Label',
        'Bin', 'Entry', 'PasswordEntry'].map(name => [name, Widget]));
    St.ButtonMask = {ONE: 1, THREE: 4, PRIMARY: 1, SECONDARY: 4};
    const Clutter = {
        Actor: Widget, ActorAlign: {}, Orientation: {},
        BinLayout: class {}, BindConstraint: class {}, BindCoordinate: {},
        KEY_Return: 13, KEY_KP_Enter: 14, KEY_ISO_Enter: 15, KEY_Escape: 27,
        EVENT_STOP: true, EVENT_PROPAGATE: false, AnimationMode: {},
    };
    const Main = {sessionMode: {isLocked: true}, uiGroup: new Widget()};
    const common = {Gio, GLib, GObject, Gdm, St, Clutter, Main,
        Pango: {EllipsizeMode: {}}, Shell: {ActionMode: {}, PolkitAuthenticationAgent: Widget},
        Signals: {EventEmitter: Emitter},
        Params: {parse: (params, defaults) => ({...defaults, ...params})},
        loadInterfaceXML: () => '<node/>',
        logErrorUnlessCancelled: error => errors.push(error),
        registerDestroyableType() {},
        ShellEntry: {addContextMenu() {}},
        AuthList: {AuthList: Widget}, WebLogin: {WebLoginDialog: Widget},
        Animation: {Spinner: Widget}, UserWidget: {},
        wiggle() {}, OVirt: {}, Vmware: {}, SmartcardManager: {},
    };
    const globals = {logError: error => errors.push(error)};
    async function source(file) {
        return readFile(join(upstream, version, 'js', file), 'utf8');
    }
    const Batch = load(await source('gdm/batch.js'), common, 'Hold,Task', globals);
    common.Batch = Batch;
    let Util, UserVerifier, AuthServicesLegacy;
    if (version !== '51.0') {
        Util = load(await source('gdm/util.js'), common, 'ShellUserVerifier,MessageType,FINGERPRINT_SERVICE_NAME', globals);
        UserVerifier = Util;
    } else {
        const MessageType = {NONE: 0, HINT: 1, INFO: 2, ERROR: 3};
        const auth = load(await source('gdm/authServices.js'), {
            ...common, MessageType, InitError: Error,
        }, 'AuthServices,Role,RoleProperties', globals);
        ({AuthServicesLegacy} = load(await source('gdm/authServicesLegacy.js'), {
            ...common, ...auth, MessageType, Settings: {}, FingerprintManager: {},
            FingerprintReaderType: {NONE: 0},
        }, 'AuthServicesLegacy', globals));
        UserVerifier = load(await source('gdm/userVerifier.js'), {
            ...common, AuthServicesLegacy, AuthServicesSSSDSwitchable: class {},
            LOGIN_SCREEN_SCHEMA: '', ALLOWED_FAILURES_KEY: '',
        }, 'ShellUserVerifier,MessageType', globals);
        Util = load(await source('gdm/util.js'), common, 'CLONE_FADE_ANIMATION_TIME', globals);
    }
    const AuthPrompt = load(await source('gdm/authPrompt.js'), {
        ...common, Util, GdmUtil: Util, UserVerifier, Atk: {StateType: {}, Live: {}},
    }, 'AuthPrompt,AuthPromptStatus', globals);
    const PolkitAgent = load(await source('ui/components/polkitAgent.js'), {
        ...common, ModalDialog: {ModalDialog: Widget}, Dialog: {},
        AccountsService: {}, PolkitAgent: {}, Polkit: {},
    }, 'Component: AuthenticationAgent,AuthenticationDialog', globals);
    // Simulate native dialog construction; its session handlers remain upstream code.
    PolkitAgent.Component.prototype._onInitiate = function () {
        this._currentDialog = this.testDialog;
    };
    const injectionSource = (await source('extensions/extension.js'))
        .slice((await source('extensions/extension.js')).indexOf('export class InjectionManager'));
    const {InjectionManager: UpstreamInjectionManager} = load(injectionSource, {}, 'InjectionManager', {...globals, Gi});
    const overrides = [];
    class InjectionManager extends UpstreamInjectionManager {
        overrideMethod(proto, name, createOverride) {
            assert.equal(typeof proto[name], 'function', `missing upstream hook: ${name}`);
            overrides.push([proto, name, proto[name]]);
            super.overrideMethod(proto, name, createOverride);
        }
    }
    class Extension {
        getSettings() { return settings; }
        gettext(text) { return text; }
    }
    const {GazeFaceAuthExtension} = load(await readFile(join(repo, 'integrations/gnome-shell/extension.js'), 'utf8'), {
        ...common, Extension, InjectionManager, gettext: _, AuthPrompt, Util, PolkitAgent,
    }, 'GazeFaceAuthExtension', globals);
    const extension = new GazeFaceAuthExtension();
    extension.enable();
    await flush();

    // Instantiate the upstream prototypes without their desktop/hardware setup.
    const verifier = Object.create(UserVerifier.ShellUserVerifier.prototype);
    Object.assign(verifier, {_settings: settings, _activeServices: new Set(),
        _messageQueue: [], _messageQueueTimeoutId: 0, _userName: 'alice',
        _userVerifier: remote, _cancellable: new Gio.Cancellable(),
        _hold: new Batch.Hold(), _defaultService: 'gdm-password',
        _fingerprintReaderType: 0, _credentialManagers: {}, _failCounter: 0,
    });
    let services;
    if (AuthServicesLegacy) {
        services = Object.create(AuthServicesLegacy.prototype);
        Object.assign(services, {_activeServices: new Set(), _unavailableServices: new Set(),
            _userName: 'alice', _userVerifier: remote, _cancellable: new Gio.Cancellable(),
            _selectedMechanism: {serviceName: 'gdm-password', role: 'password'},
            _enabledMechanisms: [{serviceName: 'gdm-password', role: 'password'}],
            _credentialManagers: {}, _enabledRoles: ['password', 'fingerprint'],
            _failCounter: 0, _allowedFailures: 3, _settings: settings,
        });
        verifier._authServices = [services];
        verifier._connectAuthServices();
        verifier._getUserVerifierProxies = async () => ({userVerifier: remote});
        const promptFactory = Object.create(AuthPrompt.AuthPrompt.prototype);
        // Exercise the extension's real factory hook without constructing a desktop.
        // The original factory constructs a verifier, so supply a constructor at
        // the native boundary that returns the prepared upstream instance.
        UserVerifier.ShellUserVerifier = class { constructor() { return verifier; } };
        assert.equal(promptFactory._createUserVerifier({}, {}), verifier);
    }
    const prompt = Object.create(AuthPrompt.AuthPrompt.prototype);
    Object.assign(prompt, {children: [], _userVerifier: verifier, _inputWell: new Widget(),
        _capsLockWarningLabel: new Widget(), _message: new Widget(),
        _timedLoginIndicator: new Widget(), _userWell: new Widget(),
        verificationStatus: AuthPrompt.AuthPromptStatus.VERIFYING, promptStep: 0,
    });
    prompt._initInputRow(); // Actual upstream widget layout, including GNOME 51's nested well.
    verifier.connect('ask-question', (_, ...args) => {
        if (version === '51.0') prompt._onAskQuestion(...args);
        else prompt._onAskQuestion(verifier, ...args);
    });
    return {version, extension, verifier, services, prompt, remote, proxy, errors,
        answers, starts, deferred, Batch, Clutter, PolkitAgent, AuthPrompt, overrides};
}

function pressKey(env, symbol) {
    const handler = env.prompt.on_key_press_event ?? env.prompt.vfunc_key_press_event;
    return handler.call(env.prompt, {get_key_symbol: () => symbol});
}

async function flush() {
    for (let i = 0; i < 8; i++) await Promise.resolve();
}

async function begin(env) {
    if (env.services) await env.verifier.begin('alice', new env.Batch.Hold());
    else env.verifier._beginVerification();
    await flush();
}

for (const version of versions) {
    test(`${version}: cold face eligibility starts authentication`, async () => {
        const env = await environment(version, {deferProbes: true});
        try {
            await begin(env);
            assert.equal(env.starts.filter(([service]) => service === 'gdm-face').length, 0);
            while (env.deferred.length) { env.deferred.shift()(); await flush(); }
            assert.deepEqual(env.starts.filter(([service]) => service === 'gdm-face'), [['gdm-face', 'alice']]);
            assert.equal(env.errors.length, 0, env.errors.map(String).join('\n'));
        } finally { env.extension.disable(); }
    });

    for (const option of ['enabled', 'enrolled', 'camera']) {
        test(`${version}: ${option}=false skips face authentication`, async () => {
            const env = await environment(version, {[option]: false});
            try {
                await begin(env);
                assert.equal(env.starts.some(([service]) => service === 'gdm-face'), false);
                assert.equal(env.errors.length, 0, env.errors.map(String).join('\n'));
            } finally { env.extension.disable(); }
        });
    }

    for (const key of ['Return', 'KP_Enter', 'ISO_Enter', 'click']) {
        test(`${version}: face confirmation via ${key} reaches PAM`, async () => {
            const env = await environment(version);
            try {
                await begin(env);
                if (env.services) env.services._onSecretInfoQuery('gdm-face', 'GAZE_REQUIRE_CONFIRMATION');
                else env.verifier._onSecretInfoQuery(null, 'gdm-face', 'GAZE_REQUIRE_CONFIRMATION');
                assert.equal(env.prompt._confirmMode, true);
                assert.equal(env.prompt._confirmButton?.visible, true);
                assert.ok(env.prompt._confirmButton.get_parent(), 'confirmation button attached');
                assert.equal(env.prompt._entry.visible, false);
                if (key === 'click') env.prompt._confirmButton.emit('clicked');
                else pressKey(env, env.Clutter[`KEY_${key}`]);
                await flush();
                assert.deepEqual(env.answers, [['gdm-face', 'GAZE_CONFIRMED']]);
                assert.equal(env.prompt._entry.reactive, false);
                assert.equal(env.errors.length, 0, env.errors.map(String).join('\n'));
            } finally { env.extension.disable(); }
        });
    }

    test(`${version}: password prompts still accept their own answers`, async () => {
        const env = await environment(version);
        try {
            await begin(env);
            if (env.services) env.services._onSecretInfoQuery('gdm-password', 'Password:');
            else env.verifier._onSecretInfoQuery(null, 'gdm-password', 'Password:');
            assert.equal(env.prompt._confirmMode, undefined);
            env.prompt._entry.text = 'test-password';
            env.prompt._activateNext();
            await flush();
            assert.deepEqual(env.answers, [['gdm-password', 'test-password']]);
            assert.equal(env.errors.length, 0, env.errors.map(String).join('\n'));
        } finally { env.extension.disable(); }
    });

    test(`${version}: disabling restores upstream hooks`, async () => {
        const env = await environment(version);
        await begin(env);
        const widgets = env.extension._activeConfirmWidgets;
        env.extension.disable();
        assert.equal(widgets.size, 0);
        assert.equal(env.extension._injectionManager, null);
        assert.equal(Object.hasOwn(Object.getPrototypeOf(env.verifier), 'serviceIsFace'), false);
        for (const [proto, name, original] of env.overrides)
            assert.equal(proto[name], original, `restored upstream hook: ${name}`);
    });

    for (const invalidate of ['disable', 'different user']) {
        test(`${version}: delayed eligibility is ignored after ${invalidate}`, async () => {
            const env = await environment(version, {deferProbes: true});
            try {
                await begin(env);
                if (invalidate === 'disable') env.extension.disable();
                else (env.services ?? env.verifier)._userName = 'bob';
                while (env.deferred.length) { env.deferred.shift()(); await flush(); }
                assert.equal(env.starts.some(([service]) => service === 'gdm-face'), false);
                assert.equal(env.errors.length, 0, env.errors.map(String).join('\n'));
            } finally { if (env.extension._injectionManager) env.extension.disable(); }
        });
    }

    test(`${version}: Escape cancels face confirmation without answering PAM`, async () => {
        const env = await environment(version);
        try {
            await begin(env);
            let cancelled = false;
            env.prompt.cancel = () => { cancelled = true; };
            if (env.services) env.services._onInfoQuery('gdm-face', 'GAZE_REQUIRE_CONFIRMATION');
            else env.verifier._onInfoQuery(null, 'gdm-face', 'GAZE_REQUIRE_CONFIRMATION');
            pressKey(env, env.Clutter.KEY_Escape);
            await flush();
            assert.equal(cancelled, true);
            assert.equal(env.prompt._confirmMode, false);
            assert.equal(env.prompt._entry.visible, true);
            assert.deepEqual(env.answers, []);
            assert.equal(env.errors.length, 0, env.errors.map(String).join('\n'));
        } finally { env.extension.disable(); }
    });

    test(`${version}: a password prompt replaces face confirmation`, async () => {
        const env = await environment(version);
        try {
            await begin(env);
            if (env.services) env.services._onSecretInfoQuery('gdm-face', 'GAZE_REQUIRE_CONFIRMATION');
            else env.verifier._onSecretInfoQuery(null, 'gdm-face', 'GAZE_REQUIRE_CONFIRMATION');
            if (env.services) env.services._onSecretInfoQuery('gdm-password', 'Password:');
            else env.verifier._onSecretInfoQuery(null, 'gdm-password', 'Password:');
            assert.equal(env.prompt._confirmMode, false);
            assert.equal(env.prompt._entry.visible, true);
            assert.equal(env.prompt._entry.reactive, true);
            env.prompt._entry.text = 'test-password';
            env.prompt._activateNext();
            await flush();
            assert.deepEqual(env.answers, [['gdm-password', 'test-password']]);
            assert.equal(env.errors.length, 0, env.errors.map(String).join('\n'));
        } finally { env.extension.disable(); }
    });

    for (const entry of ['Enter', 'Authenticate button', 'password']) {
        test(`${version}: Polkit ${entry} sends the expected PAM answer`, async () => {
            const env = await environment(version);
            try {
                const dialog = Object.create(env.PolkitAgent.AuthenticationDialog.prototype);
                const session = new Emitter();
                const responses = [];
                session.response = answer => responses.push(answer);
                Object.assign(dialog, {_session: session, _passwordEntry: new Widget(),
                    _infoMessageLabel: new Widget(), _errorMessageLabel: new Widget(),
                    _nullMessageLabel: new Widget(), _okButton: new Widget(),
                    _ensureOpen() {},
                });
                new Widget().add_child(dialog._passwordEntry);
                const component = Object.create(env.PolkitAgent.Component.prototype);
                component.testDialog = dialog;
                component._onInitiate();
                session.emit('request', entry === 'password' ? 'Password:' : 'GAZE_REQUIRE_CONFIRMATION', false);
                if (entry === 'password') dialog._passwordEntry.text = 'test-password';
                if (entry === 'Authenticate button') dialog._onAuthenticateButtonPressed();
                else dialog._onEntryActivate();
                assert.deepEqual(responses, [entry === 'password' ? 'test-password' : 'GAZE_CONFIRMED']);
                assert.equal(env.errors.length, 0, env.errors.map(String).join('\n'));
                const handler = env.extension._originalDialogRequest;
                env.extension.disable();
                assert.equal(Object.getPrototypeOf(dialog)._onSessionRequest, handler);
            } finally { if (env.extension._injectionManager) env.extension.disable(); }
        });
    }
}

for (const retryMode of ['disabled', 'fixed', 'infinite']) {
    test(`51.0: ${retryMode} face retries preserve password authentication`, async () => {
        const env = await environment('51.0', {retryMode});
        try {
            await begin(env);
            for (let i = 0; i < 3; i++) {
                env.services._onConversationStopped('gdm-face');
                await flush();
            }
            const expected = {disabled: 1, fixed: 2, infinite: 4}[retryMode];
            assert.equal(env.starts.filter(([service]) => service === 'gdm-face').length, expected);
            assert.equal(env.verifier.events?.some(([name]) => name === 'verification-failed') ?? false, false);
            env.services._onSecretInfoQuery('gdm-password', 'Password:');
            env.prompt._entry.text = 'test-password';
            env.prompt._activateNext();
            await flush();
            assert.deepEqual(env.answers, [['gdm-password', 'test-password']]);
            assert.equal(env.errors.length, 0, env.errors.map(String).join('\n'));
        } finally { env.extension.disable(); }
    });
}

test('51.0: cancelled confirmation does not send a delayed answer', async () => {
    const env = await environment('51.0');
    try {
        await begin(env);
        let releaseMessages;
        env.verifier.handlePendingMessages = () => new Promise(resolve => { releaseMessages = resolve; });
        env.services._onSecretInfoQuery('gdm-face', 'GAZE_REQUIRE_CONFIRMATION');
        env.prompt._confirmButton.emit('clicked');
        assert.deepEqual(env.answers, []);
        env.prompt.cancel = () => {};
        env.prompt._confirmMode = true;
        pressKey(env, env.Clutter.KEY_Escape);
        releaseMessages();
        await flush();
        assert.deepEqual(env.answers, []);
        assert.equal(env.errors.length, 0, env.errors.map(String).join('\n'));
    } finally { env.extension.disable(); }
});

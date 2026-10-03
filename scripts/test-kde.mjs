// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

// Run with: node scripts/test-kde.mjs [upstream-source-directory]
// Downloads the KDE PAM stacks each supported distribution ships when no source
// directory is given, runs gaze-kde-pam against them, and evaluates the results
// with Linux-PAM's dispatch rules. PAM modules are simulated; this is not a
// replacement for unlocking an actual Plasma session.
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import {existsSync} from 'node:fs';
import {cp, mkdir, mkdtemp, readFile, readdir, rm, writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {dirname, join, relative} from 'node:path';
import {fileURLToPath} from 'node:url';
import {gunzipSync, zstdDecompressSync} from 'node:zlib';
import {test, after} from 'node:test';

const repo = fileURLToPath(new URL('../', import.meta.url));
const helper = join(repo, 'integrations/kde/gaze-kde-pam');
const BEGIN = '# BEGIN gaze (managed by gaze-kde; remove with `gaze-kde-pam disable`)';
const END = '# END gaze';
const SLOTS = ['kde', 'kde-fingerprint', 'kde-smartcard'];
const LOCK_SLOTS = ['kde-fingerprint', 'kde-smartcard'];
const LOGIN = ['plasmalogin', 'sddm'];

const ARCH = 'https://gitlab.archlinux.org/archlinux/packaging/packages';
const FEDORA = 'https://src.fedoraproject.org/rpms';
const DEBIAN = 'https://sources.debian.org';
const UBUNTU = 'https://git.launchpad.net/ubuntu/+source';
// openSUSE's own servers refuse GitHub's runners, so fall back to public mirrors.
const SUSE_MIRRORS = ['https://download.opensuse.org/tumbleweed/repo/oss',
    'https://ftp.fau.de/opensuse/tumbleweed/repo/oss', 'https://ftp.gwdg.de/pub/opensuse/tumbleweed/repo/oss'];
const KDE = 'https://invent.kde.org/plasma';

// Every host here throttles bursts from shared CI addresses, so cap and retry.
const slots = Array(6).fill(Promise.resolve());
let nextSlot = 0;

async function download(url, {optional = false, binary = false} = {}) {
    const slot = nextSlot++ % slots.length;
    const result = slots[slot].then(() => fetchWithRetry(url, optional, binary));
    slots[slot] = result.catch(() => {});
    return result;
}

async function fetchWithRetry(url, optional, binary) {
    for (let attempt = 1; ; attempt++) {
        try {
            const response = await fetch(url, {signal: AbortSignal.timeout(60_000)});
            if (optional && response.status === 404) return null;
            if (response.status === 200) return binary ? Buffer.from(await response.arrayBuffer()) : await response.text();
            if (response.status < 500 && response.status !== 429) assert.fail(`${url}: HTTP ${response.status}`);
            throw new Error(`HTTP ${response.status}`);
        } catch (error) {
            if (error instanceof assert.AssertionError) throw error;
            if (attempt === 4) throw new Error(`${url}: ${error.message}`, {cause: error});
            await new Promise(resolve => setTimeout(resolve, 2_000 * attempt));
        }
    }
}

function field(source, pattern, url) {
    const match = source.match(pattern);
    assert.ok(match, `${url}: ${pattern}`);
    return match[1];
}

async function debianSource(pkg, suite) {
    const url = `${DEBIAN}/api/src/${pkg}/`;
    const {versions} = JSON.parse(await download(url));
    const found = versions.find(version => version.suites.includes(suite));
    assert.ok(found, `${pkg} in ${suite}`);
    return `${DEBIAN}/data/main/${pkg[0]}/${pkg}/${found.version}`;
}

const arch = async () => {
    const plmUrl = `${ARCH}/plasma-login-manager/-/raw/main/PKGBUILD`;
    const sddmUrl = `${ARCH}/sddm/-/raw/main/PKGBUILD`;
    const plm = field(await download(plmUrl), /^pkgver=(\S+)/m, plmUrl);
    const sddm = field(await download(sddmUrl), /^pkgver=(\S+)/m, sddmUrl);
    return [
        ...SLOTS.map(name => [`vendor/${name}`, `${ARCH}/kscreenlocker/-/raw/main/${name}.pam`]),
        ['vendor/plasmalogin', `${KDE}/plasma-login-manager/-/raw/v${plm}/data/pam/arch/plasmalogin`],
        ['etc/sddm', `https://raw.githubusercontent.com/sddm/sddm/v${sddm}/services/sddm.pam`],
        ...['system-auth', 'system-login', 'system-local-login']
            .map(name => [`etc/${name}`, `${ARCH}/pambase/-/raw/main/${name}`]),
    ];
};

const fedora = branch => async () => {
    const authselectUrl = `${FEDORA}/authselect/raw/${branch}/f/authselect.spec`;
    const authselect = field(await download(authselectUrl), /^Version:\s*(\S+)/m, authselectUrl);
    const plmUrl = `${FEDORA}/plasma-login-manager/raw/${branch}/f/plasma-login-manager.spec`;
    const plmSpec = await download(plmUrl);
    const plm = plmSpec.match(/^%global commit (\S+)/m)?.[1] ?? `v${field(plmSpec, /^Version:\s*(\S+)/m, plmUrl)}`;
    return [
        ...SLOTS.map(name => [`etc/${name}`, `${FEDORA}/plasma-workspace/raw/${branch}/f/${name}`]),
        ['etc/sddm', `${FEDORA}/sddm/raw/${branch}/f/sddm.pam`],
        ['vendor/plasmalogin', `${KDE}/plasma-login-manager/-/raw/${plm}/data/pam/fedora/plasmalogin`],
        ...['password-auth', 'fingerprint-auth', 'smartcard-auth', 'postlogin'].map(name => [`authselect/${name}`,
            `https://raw.githubusercontent.com/authselect/authselect/${authselect}/profiles/local/${name}`]),
    ];
};

const debian = suite => async () => {
    const [kscreenlocker, sddm, pam] = await Promise.all(['kscreenlocker', 'sddm', 'pam']
        .map(pkg => debianSource(pkg, suite)));
    return [
        ...SLOTS.map(name => [`etc/${name}`, `${kscreenlocker}/debian/pam.d/${name}`]),
        ['etc/sddm', `${sddm}/debian/sddm.pam`],
        ['debian/common-auth', `${pam}/debian/local/common-auth`],
        ['debian/unix', `${pam}/debian/pam-configs/unix`],
    ];
};

const ubuntu = series => async () => {
    const at = (pkg, path) => `${UBUNTU}/${pkg}/plain/${path}?h=ubuntu/${series}`;
    return [
        // Plasma 5 builds ship no biometric slots; gaze-kde-pam creates one.
        ...SLOTS.map(name => [`etc/${name}`, at('kscreenlocker', `debian/pam.d/${name}`), true]),
        ['etc/sddm', at('sddm', 'debian/sddm.pam')],
        ['debian/common-auth', at('pam', 'debian/local/common-auth')],
        ['debian/unix', at('pam', 'debian/pam-configs/unix')],
    ];
};

function rpmFiles(buffer) {
    const header = offset => {
        assert.equal(buffer.readUInt32BE(offset), 0x8eade801, 'RPM header magic');
        const count = buffer.readUInt32BE(offset + 8);
        const store = offset + 16 + count * 16;
        const tags = new Map();
        for (let i = 0; i < count; i++)
            tags.set(buffer.readUInt32BE(offset + 16 + i * 16), store + buffer.readUInt32BE(offset + 24 + i * 16));
        return {end: store + buffer.readUInt32BE(offset + 12),
            string: tag => tags.has(tag) ? buffer.toString('utf8', tags.get(tag), buffer.indexOf(0, tags.get(tag))) : null};
    };
    const signature = header(96);
    const main = header(Math.ceil(signature.end / 8) * 8);
    const compressor = main.string(1125) ?? 'gzip';
    const decompress = {zstd: zstdDecompressSync, gzip: gunzipSync}[compressor];
    assert.ok(decompress, `unsupported RPM payload compressor: ${compressor}`);
    const cpio = decompress(buffer.subarray(main.end));
    const files = new Map();
    for (let offset = 0; ;) {
        assert.equal(cpio.toString('ascii', offset, offset + 6), '070701', 'cpio newc magic');
        const field = i => parseInt(cpio.toString('ascii', offset + 6 + i * 8, offset + 14 + i * 8), 16);
        const size = field(6), nameSize = field(11);
        const name = cpio.toString('utf8', offset + 110, offset + 109 + nameSize);
        if (name === 'TRAILER!!!') return files;
        const data = Math.ceil((offset + 110 + nameSize) / 4) * 4;
        files.set(name.replace(/^\.?\/?/, '/'), cpio.subarray(data, data + size));
        offset = Math.ceil((data + size) / 4) * 4;
    }
}

async function suseRpm(pkg) {
    const pattern = new RegExp(`(?<![\\w.+-])${pkg.replace(/[.+]/g, '\\$&')}-\\d[^"<>/\\s-]*-[^"<>/\\s-]+\\.x86_64\\.rpm`, 'g');
    const errors = [];
    for (const mirror of SUSE_MIRRORS) {
        try {
            const names = [...new Set((await download(`${mirror}/x86_64/?P=${pkg}-*`)).match(pattern) ?? [])]
                .sort((a, b) => a.localeCompare(b, undefined, {numeric: true}));
            assert.ok(names.length, `no ${pkg} package listed`);
            return rpmFiles(await download(`${mirror}/x86_64/${names.at(-1)}`, {binary: true}));
        } catch (error) {
            errors.push(`${mirror}: ${error.message}`);
        }
    }
    throw new Error(`${pkg}: ${errors.join('; ')}`);
}

const tumbleweed = async () => {
    const [kscreenlocker, sddm, pam] = await Promise.all(['kscreenlocker6', 'sddm-qt6', 'pam'].map(suseRpm));
    const file = (files, path) => {
        assert.ok(files.has(path), `missing ${path}`);
        return {content: files.get(path).toString('utf8')};
    };
    return [
        ...SLOTS.map(name => [`vendor/${name}`, file(kscreenlocker, `/usr/lib/pam.d/${name}`)]),
        ['vendor/sddm', file(sddm, '/usr/lib/pam.d/sddm')],
        ['vendor/common-auth', file(pam, '/usr/lib/pam.d/common-auth')],
    ];
};

const targets = {
    'arch': {files: arch},
    'debian-trixie': {files: debian('trixie')},
    'debian-forky': {files: debian('forky')},
    'fedora-42': {files: fedora('f42')},
    'fedora-43': {files: fedora('f43')},
    'fedora-44': {files: fedora('f44')},
    'opensuse-tumbleweed': {files: tumbleweed, modules: ['pam_pkcs11.so']},
    'ubuntu-noble': {files: ubuntu('noble')},
    'ubuntu-questing': {files: ubuntu('questing')},
    'ubuntu-resolute': {files: ubuntu('resolute')},
    'ubuntu-stonking': {files: ubuntu('stonking')},
};

const upstream = process.argv[2] ?? await mkdtemp(join(tmpdir(), 'gaze-kde-'));
if (!process.argv[2]) {
    after(() => rm(upstream, {recursive: true, force: true}));
    const downloads = await Promise.allSettled(Object.entries(targets).map(async ([name, target]) => {
        await Promise.all((await target.files()).map(async ([file, url, optional]) => {
            const body = url.content ?? await download(url, {optional});
            if (body === null) return;
            assert.doesNotMatch(body, /^\s*</, `${url}: got an HTML page instead of a source file`);
            const destination = join(upstream, name, file);
            await mkdir(dirname(destination), {recursive: true});
            await writeFile(destination, body);
        }));
    }));
    for (const result of downloads)
        if (result.status === 'rejected') throw result.reason;
}

function renderAuthselect(source, features) {
    const lines = [];
    for (let line of source.split('\n')) {
        const flow = line.match(/^\s*\{(continue|stop) if "([^"]+)"\}\s*$/);
        if (flow) {
            if ((flow[1] === 'continue') !== features.includes(flow[2])) break;
            continue;
        }
        const condition = line.match(/\s*\{(include|exclude) if "([^"]+)"\}\s*$/);
        if (condition) {
            if ((condition[1] === 'include') !== features.includes(condition[2])) continue;
            line = line.slice(0, condition.index);
        }
        lines.push(line.replace(/\{if (not )?"([^"]+)":([^|}]*)(?:\|([^}]*))?\}/g,
            (_, not, feature, yes, no = '') => features.includes(feature) !== Boolean(not) ? yes : no).trimEnd());
    }
    const rendered = lines.join('\n');
    assert.doesNotMatch(rendered, /\{(include|exclude|if|imply|continue|stop)\b/, 'unrendered authselect directive');
    return rendered;
}

// What pam-auth-update writes when pam_unix is the only primary module.
function renderCommonAuth(template, unix) {
    const initial = field(unix, /^Auth-Initial:\n((?:[ \t]+.*\n?)+)/m, 'pam-configs/unix').trim().split('\n')
        .map(line => `auth\t${line.trim().replace('success=end', 'success=1')}`);
    assert.match(template, /\$auth_primary/);
    return template.replace('$auth_primary', initial.join('\n')).replace('$auth_additional', '');
}

const KEYWORDS = {
    required: {success: 'ok', new_authtok_reqd: 'ok', ignore: 'ignore', default: 'bad'},
    requisite: {success: 'ok', new_authtok_reqd: 'ok', ignore: 'ignore', default: 'die'},
    optional: {success: 'ok', new_authtok_reqd: 'ok', default: 'ignore'},
    sufficient: {success: 'done', new_authtok_reqd: 'done', default: 'ignore'},
};

function actions(control) {
    if (KEYWORDS[control]) return KEYWORDS[control];
    assert.match(control, /^\[.*\]$/, `PAM control ${control}`);
    const parsed = {default: 'bad'};
    for (const pair of control.slice(1, -1).trim().split(/\s+/)) {
        const [code, action] = pair.split('=');
        parsed[code] = /^\d+$/.test(action) ? Number(action) : action;
    }
    return parsed;
}

function parse(source) {
    const lines = [];
    let managed = false;
    for (const raw of source.split('\n')) {
        if (raw === BEGIN) { managed = true; continue; }
        if (raw === END) { managed = false; continue; }
        const line = raw.replace(/#.*/, '').trim();
        if (!line) continue;
        if (line.startsWith('@include')) {
            lines.push({type: '@include', target: line.split(/\s+/)[1]});
            continue;
        }
        const match = line.match(/^-?(\w+)\s+(\[[^\]]*\]|\S+)\s+(\S+)\s*(.*)$/);
        assert.ok(match, `unparsable PAM line: ${line}`);
        lines.push({type: match[1], control: match[2], module: match[3],
            args: match[4].split(/\s+/).filter(Boolean), managed});
    }
    return lines;
}

// Modules a stock install may lack; everything else counts as installed.
const OPTIONAL_MODULES = new Set(['pam_gaze.so', 'pam_fprintd.so', 'pam_pkcs11.so']);

function moduleResult({module, args}, scenario) {
    switch (module) {
    case 'pam_gaze.so': return scenario.face ?? 'authinfo_unavail';
    case 'pam_unix.so':
    case 'pam_unix2.so':
        scenario.prompted = true;
        return scenario.password === 'right' ? 'success' : 'auth_err';
    case 'pam_sss.so': return 'authinfo_unavail';
    case 'pam_fprintd.so': return scenario.finger ?? 'auth_err';
    case 'pam_pkcs11.so':
        if (args.includes('wait_for_card')) scenario.blocked = true;
        return 'authinfo_unavail';
    case 'pam_faillock.so':
        if (args.includes('preauth')) return scenario.locked ? 'auth_err' : 'success';
        if (args.includes('authfail')) { scenario.counted = true; return 'auth_err'; }
        return 'success';
    case 'pam_nologin.so': return scenario.nologin ? 'auth_err' : 'success';
    case 'pam_deny.so': return 'auth_err';
    case 'pam_debug.so': return args.find(arg => arg.startsWith('auth='))?.slice(5) ?? 'success';
    case 'pam_selinux_permit.so': return 'ignore';
    case 'pam_systemd_home.so': return 'user_unknown';
    case 'pam_permit.so': case 'pam_shells.so': case 'pam_env.so': case 'pam_succeed_if.so':
    case 'pam_faildelay.so': case 'pam_kwallet5.so': case 'pam_kwallet.so': case 'pam_gnome_keyring.so':
        return 'success';
    default: throw new Error(`unmodelled PAM module: ${module}`);
    }
}

// Mirrors _pam_dispatch_aux in libpam/pam_dispatch.c: includes are inlined,
// substacks keep their own level, and a missing module (with or without the
// `-` prefix, which only silences logging) returns PAM_MODULE_UNKNOWN.
function dispatch(handlers, scenario, installed) {
    const substates = [];
    let impression = 'undef', status = 'perm_denied', previous = 0;
    const skipLevel = (i, level) => {
        while (i + 1 < handlers.length && handlers[i + 1].level >= level) i++;
        return i;
    };
    for (let i = 0; i < handlers.length; i++) {
        const handler = handlers[i];
        const level = handler.level;
        if (previous < level) substates[level] = {impression, status};
        previous = level;
        if (handler.substack) continue;
        const result = installed(handler.module) ? moduleResult(handler, scenario) : 'module_unknown';
        scenario.calls.push({module: handler.module, args: handler.args, managed: handler.managed, result});
        if (scenario.blocked) return 'blocked';
        const action = handler.actions[result] ?? handler.actions.default;
        if (action === 'reset') {
            ({impression, status} = substates[level]);
        } else if (action === 'ok' || action === 'done') {
            if ((impression === 'undef' || (impression === 'positive' && status === 'success')) && result !== 'ignore') {
                impression = 'positive';
                status = result;
            }
            if (impression === 'positive' && action === 'done') i = skipLevel(i, level);
        } else if (action === 'bad' || action === 'die') {
            if (impression !== 'negative') {
                impression = 'negative';
                status = result === 'ignore' ? 'perm_denied' : result;
            }
            if (action === 'die') i = skipLevel(i, level);
        } else if (typeof action === 'number') {
            let jump = action;
            while (i + 1 < handlers.length && handlers[i + 1].level >= level && jump > 0) {
                do i++; while (i + 1 < handlers.length && handlers[i + 1].level > level);
                jump--;
            }
            if (jump) { impression = 'negative'; status = 'perm_denied'; }
        } else {
            assert.equal(action, 'ignore', `PAM action ${action}`);
        }
    }
    return status === 'success' && impression !== 'positive' ? 'perm_denied' : status;
}

async function environment(target, features, {reader = false} = {}) {
    const root = await mkdtemp(join(tmpdir(), 'gaze-kde-env-'));
    const etc = join(root, 'etc'), vendor = join(root, 'vendor');
    const state = join(root, 'state'), security = join(root, 'security');
    for (const dir of [etc, vendor, security]) await mkdir(dir, {recursive: true});
    const source = join(upstream, target);
    for (const dir of ['etc', 'vendor'])
        if (existsSync(join(source, dir))) await cp(join(source, dir), join(root, dir), {recursive: true});
    if (existsSync(join(source, 'authselect')))
        for (const name of await readdir(join(source, 'authselect')))
            await writeFile(join(etc, name), renderAuthselect(await readFile(join(source, 'authselect', name), 'utf8'), features));
    if (existsSync(join(source, 'debian'))) {
        await writeFile(join(etc, 'common-auth'), renderCommonAuth(
            await readFile(join(source, 'debian/common-auth'), 'utf8'),
            await readFile(join(source, 'debian/unix'), 'utf8')));
        // pam-auth-update never puts auth lines in these; the stacks @include them whole.
        for (const name of ['common-account', 'common-session', 'common-password'])
            await writeFile(join(etc, name), '');
    }
    const modules = new Set(['pam_gaze.so', ...(targets[target].modules ?? []), ...(reader ? ['pam_fprintd.so'] : [])]);
    for (const module of modules) await writeFile(join(security, module), '');
    const env = {...process.env,
        GAZE_KDE_PAM_FILE: join(etc, 'kde-fingerprint'),
        GAZE_KDE_SMARTCARD_PAM_FILE: join(etc, 'kde-smartcard'),
        GAZE_KDE_LOGIN_PAM_FILES: LOGIN.map(name => join(etc, name)).join(' '),
        GAZE_KDE_LOGIN_FACE_PAM_FILE: join(etc, 'plasmalogin-fingerprint'),
        GAZE_KDE_VENDOR_PAM_DIR: vendor,
        GAZE_KDE_STATE_DIR: state,
        GAZE_KDE_SECURITY_DIRS: security,
    };
    const locate = name => [join(etc, name), join(vendor, name)].find(existsSync);
    const read = async name => locate(name) ? readFile(locate(name), 'utf8') : null;

    async function handlers(name, level = 0, out = []) {
        const file = locate(name);
        assert.ok(file, `${target}: missing PAM stack ${name}`);
        for (const line of parse(await readFile(file, 'utf8'))) {
            if (line.type === '@include') await handlers(line.target, level, out);
            else if (line.type !== 'auth') continue;
            else if (line.control === 'include') await handlers(line.module, level, out);
            else if (line.control === 'substack') {
                out.push({level, substack: true});
                await handlers(line.module, level + 1, out);
            } else out.push({...line, level, actions: actions(line.control)});
        }
        return out;
    }

    return {
        target, etc, vendor,
        exists: name => Boolean(locate(name)),
        read,
        run(...args) {
            const result = spawnSync('sh', [helper, ...args], {env, encoding: 'utf8'});
            assert.equal(result.status, 0, `gaze-kde-pam ${args.join(' ')}\n${result.stdout}${result.stderr}`);
            return result.stdout;
        },
        status: () => spawnSync('sh', [helper, 'status'], {env, encoding: 'utf8'}).stdout,
        async wired() {
            const wired = [];
            for (const name of LOCK_SLOTS)
                if ((await read(name))?.includes(BEGIN)) wired.push(name);
            return wired;
        },
        async files() {
            const files = {};
            for (const dir of [etc, vendor])
                for (const entry of await readdir(dir, {recursive: true, withFileTypes: true}))
                    if (entry.isFile()) {
                        const path = join(entry.parentPath, entry.name);
                        files[relative(root, path)] = await readFile(path, 'utf8');
                    }
            return files;
        },
        handlers,
        async authenticate(name, options = {}) {
            const scenario = {...options, calls: [], prompted: false, counted: false, blocked: false};
            const installed = module => !OPTIONAL_MODULES.has(module) || modules.has(module);
            const status = dispatch(await handlers(name), scenario, installed);
            return {status, prompted: scenario.prompted, counted: scenario.counted,
                calls: scenario.calls.filter(call => !call.managed)
                    .map(({module, args, result}) => `${module} ${args.join(' ')} -> ${result}`)};
        },
        cleanup: () => rm(root, {recursive: true, force: true}),
    };
}

async function withEnvironment(target, features, options, body) {
    const env = await environment(target, features, options);
    try { return await body(env); } finally { await env.cleanup(); }
}

const variants = target => target.startsWith('fedora-')
    ? [['with-silent-lastlog'], ['with-silent-lastlog', 'with-fingerprint', 'with-faillock']]
    : [[]];
const NON_MATCHES = ['auth_err', 'authinfo_unavail', 'ignore'];

for (const target of Object.keys(targets)) {
    for (const features of variants(target)) {
        const extra = features.filter(feature => feature !== 'with-silent-lastlog');
        const label = extra.length ? `${target} (${extra.join(' ')})` : target;
        const scoped = (name, body, options = {}) =>
            test(`${label}: ${name}`, () => withEnvironment(target, features, options, body));

        scoped('face unlock opens the lock screen without touching the password service', async env => {
            const password = await env.read('kde');
            env.run('enable');
            const wired = await env.wired();
            assert.equal(wired.length, 1, `wired slots: ${wired}`);
            const result = await env.authenticate(wired[0], {face: 'success'});
            assert.equal(result.status, 'success', result.calls.join('\n'));
            assert.equal(result.prompted, false);
            assert.equal(result.counted, false);
            assert.equal(await env.read('kde'), password);
            assert.match(env.status(), /lock screen: enabled/);
        });

        scoped('locked and nologin accounts stay locked on a face match', async env => {
            env.run('enable');
            const [slot] = await env.wired();
            for (const gate of ['locked', 'nologin']) {
                const result = await env.authenticate(slot, {face: 'success', [gate]: true});
                assert.notEqual(result.status, 'success', `${gate}\n${result.calls.join('\n')}`);
            }
        });

        scoped('a face non-match never unlocks or counts as a failed login', async env => {
            env.run('enable');
            const [slot] = await env.wired();
            for (const face of NON_MATCHES) {
                const result = await env.authenticate(slot, {face});
                assert.notEqual(result.status, 'success', `${face}\n${result.calls.join('\n')}`);
                assert.equal(result.counted, false, `${face} reached pam_faillock authfail`);
            }
        });

        scoped('a face non-match leaves the distribution stack in charge', async env => {
            const original = Object.fromEntries(await Promise.all(LOCK_SLOTS.map(async name =>
                [name, env.exists(name) ? await env.authenticate(name, {face: 'auth_err'}) : null])));
            env.run('enable');
            const [slot] = await env.wired();
            const block = (await env.read(slot)).split(BEGIN)[1].split(END)[0];
            if (!original[slot] || /default=die/.test(block)) return;
            for (const face of NON_MATCHES)
                assert.deepEqual(await env.authenticate(slot, {face}), original[slot], face);
        });

        scoped('disable restores every distribution file', async env => {
            const before = await env.files();
            env.run('enable');
            env.run('disable');
            assert.deepEqual(await env.files(), before);
            assert.match(env.status(), /lock screen: (disabled|not configured)/);
        });

        scoped('enable is idempotent and an explicit disable outlives upgrades', async env => {
            const before = await env.files();
            env.run('enable');
            const enabled = await env.files();
            env.run('enable');
            assert.deepEqual(await env.files(), enabled);
            env.run('disable');
            env.run('enable');
            assert.deepEqual(await env.files(), before);
            env.run('enable', '--force');
            assert.deepEqual(await env.files(), enabled);
        });

        scoped('a fingerprint reader keeps its own slot', async env => {
            const readerSlot = await env.read('kde-fingerprint');
            const runsReader = env.exists('kde-fingerprint') &&
                (await env.handlers('kde-fingerprint')).some(handler => handler.module === 'pam_fprintd.so');
            const expected = runsReader && env.exists('kde-smartcard') ? 'kde-smartcard' : 'kde-fingerprint';
            const finger = runsReader ? await env.authenticate('kde-fingerprint', {finger: 'success'}) : null;
            env.run('enable');
            assert.deepEqual(await env.wired(), [expected]);
            if (expected === 'kde-smartcard') {
                assert.equal(await env.read('kde-fingerprint'), readerSlot);
                assert.deepEqual(await env.authenticate('kde-fingerprint', {finger: 'success'}), finger);
            }
            assert.equal((await env.authenticate(expected, {face: 'success'})).status, 'success');
        }, {reader: true});

        scoped('login greeters accept a face or the password', async env => {
            const services = LOGIN.filter(env.exists);
            assert.ok(services.length, 'no login greeter stack');
            env.run('enable-login');
            for (const service of services) {
                const cases = [
                    [{face: 'success'}, true],
                    [{face: 'success', locked: true}, false],
                    [{face: 'success', nologin: true}, false],
                    ...NON_MATCHES.flatMap(face => [[{face, password: 'right'}, true], [{face, password: 'wrong'}, false]]),
                ];
                for (const [scenario, unlocks] of cases) {
                    const result = await env.authenticate(service, scenario);
                    assert.equal(result.status === 'success', unlocks,
                        `${service} ${JSON.stringify(scenario)}\n${result.calls.join('\n')}`);
                    if (scenario.face === 'success' && unlocks) assert.equal(result.prompted, false);
                }
            }
            assert.match(env.status(), /login greeter: enabled/);
        });

        scoped('disable-login restores the login stacks', async env => {
            const before = await env.files();
            env.run('enable-login');
            env.run('disable-login');
            assert.deepEqual(await env.files(), before);
        });
    }
}

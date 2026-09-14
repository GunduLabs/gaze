# Copyright 2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v2

EAPI=8

CRATES="
	adler2@2.0.1
	aead@0.6.1
	aes-gcm@0.11.1
	aes@0.9.3
	aho-corasick@1.1.5
	allocator-api2@0.2.21
	anstream@1.0.0
	anstyle-parse@1.0.0
	anstyle-query@1.1.5
	anstyle-wincon@3.0.11
	anstyle@1.0.14
	anyhow@1.0.104
	approx@0.5.1
	async-broadcast@0.7.2
	async-channel@2.5.0
	async-executor@1.14.0
	async-io@2.6.0
	async-lock@3.4.2
	async-process@2.5.0
	async-recursion@1.1.1
	async-signal@0.2.14
	async-task@4.7.1
	async-trait@0.1.92
	atomic_refcell@0.1.14
	atomic-waker@1.1.2
	atomic@0.6.1
	autocfg@1.5.1
	base64@0.22.1
	base64@0.23.1
	base64ct@1.8.3
	bit-set@0.5.3
	bit-vec@0.6.3
	bitfield-macros@0.19.5
	bitfield@0.19.5
	bitflags@1.3.2
	bitflags@2.13.1
	block-buffer@0.10.4
	block-buffer@0.12.1
	blocking@1.7.0
	bumpalo@3.20.3
	by_address@1.2.1
	bytemuck@1.25.2
	byteorder-lite@0.1.0
	byteorder@1.5.0
	bytes@1.12.1
	bzip2@0.6.1
	cairo-rs@0.22.9
	cairo-sys-rs@0.22.9
	castaway@0.2.4
	cc@1.4.5
	cfg_aliases@0.2.2
	cfg-expr@0.20.9
	cfg-if@1.0.4
	cipher@0.5.2
	clang-sys@1.9.1
	clang@2.1.0
	clap_builder@4.6.6
	clap_complete@4.6.9
	clap_derive@4.6.4
	clap_lex@1.1.0
	clap@4.6.6
	cmov@0.5.4
	colorchoice@1.0.5
	compact_str@0.9.1
	concurrent-queue@2.5.0
	console@0.16.4
	const-oid@0.10.2
	constant_time_eq@0.4.2
	convert_case@0.10.0
	core-foundation-sys@0.8.7
	core-foundation@0.10.1
	cpubits@0.1.1
	cpufeatures@0.2.17
	cpufeatures@0.3.1
	crc32fast@1.5.1
	critical-section@1.2.0
	crossbeam-utils@0.8.23
	crossterm_winapi@0.9.1
	crossterm@0.29.0
	crypto-common@0.1.7
	crypto-common@0.2.2
	csscolorparser@0.6.2
	ctr@0.10.1
	ctutils@0.4.2
	darling_core@0.24.1
	darling_macro@0.24.1
	darling@0.24.1
	deflate64@0.1.12
	deltae@0.3.2
	der@0.8.2
	deranged@0.5.8
	derive_more-impl@2.1.1
	derive_more@2.1.1
	dialoguer@0.12.0
	digest@0.10.7
	digest@0.11.3
	document-features@0.2.12
	dunce@1.0.5
	either@1.18.0
	encode_unicode@1.0.0
	endi@1.1.1
	enumflags2_derive@0.7.12
	enumflags2@0.7.12
	equivalent@1.0.2
	errno@0.3.14
	euclid@0.22.14
	event-listener-strategy@0.5.4
	event-listener@5.4.2
	fancy-regex@0.11.0
	fastrand@2.5.0
	field-offset@0.3.6
	filedescriptor@0.8.3
	find-msvc-tools@0.1.12
	finl_unicode@1.4.0
	fixedbitset@0.4.2
	flate2@1.1.10
	fnv@1.0.7
	foldhash@0.2.0
	foreign-types-shared@0.1.1
	foreign-types@0.3.2
	futures-channel@0.3.34
	futures-core@0.3.34
	futures-executor@0.3.34
	futures-io@0.3.34
	futures-lite@2.6.1
	futures-macro@0.3.34
	futures-sink@0.3.34
	futures-task@0.3.34
	futures-util@0.3.34
	futures@0.3.34
	gdk-pixbuf-sys@0.22.9
	gdk-pixbuf@0.22.0
	gdk4-sys@0.11.4
	gdk4@0.11.4
	generic-array@0.14.7
	getrandom@0.2.17
	getrandom@0.3.4
	getrandom@0.4.3
	ghash@0.6.0
	gio-sys@0.22.9
	gio@0.22.9
	glam@0.30.10
	glam@0.31.1
	glam@0.32.1
	glam@0.33.6
	glib-macros@0.22.9
	glib-sys@0.22.9
	glib@0.22.9
	glob@0.3.4
	gobject-sys@0.22.9
	graphene-rs@0.22.8
	graphene-sys@0.22.9
	gsk4-sys@0.11.4
	gsk4@0.11.4
	gstreamer-app-sys@0.25.0
	gstreamer-app@0.25.2
	gstreamer-base-sys@0.25.3
	gstreamer-base@0.25.3
	gstreamer-sys@0.25.2
	gstreamer-video-sys@0.25.3
	gstreamer-video@0.25.3
	gstreamer@0.25.3
	gtk4-macros@0.11.4
	gtk4-sys@0.11.4
	gtk4@0.11.4
	hashbrown@0.16.1
	hashbrown@0.17.1
	heck@0.5.0
	hermit-abi@0.5.3
	hex@0.4.3
	hmac-sha256@1.1.14
	hmac@0.13.0
	hostname-validator@1.1.1
	http@1.5.0
	httparse@1.10.1
	hybrid-array@0.4.14
	ident_case@1.0.1
	image@0.25.10
	indexmap@2.14.2
	indoc@2.0.7
	inout@0.2.2
	instability@0.3.13
	is_executable@1.0.6
	is_terminal_polyfill@1.70.2
	itertools@0.14.0
	itertools@0.15.0
	itoa@1.0.18
	jobserver@0.1.35
	js-sys@0.3.105
	kasuari@0.4.12
	kstring@2.0.4
	lab@0.11.0
	lazy_static@1.5.0
	libadwaita-sys@0.9.2
	libadwaita@0.9.2
	libbz2-rs-sys@0.2.5
	libc@0.2.189
	libm@0.2.16
	line-clipping@0.3.8
	linux-raw-sys@0.12.1
	litrs@1.0.0
	lock_api@0.4.14
	log@0.4.34
	lru@0.18.4
	lzma-rust2@0.15.8
	lzma-rust2@0.16.5
	mac_address@1.1.8
	matchers@0.2.0
	matrixmultiply@0.3.11
	mbox@0.7.1
	memchr@2.8.3
	memmem@0.1.1
	memoffset@0.9.1
	minimal-lexical@0.2.1
	miniz_oxide@0.9.1
	mio@1.2.3
	moxcms@0.8.1
	muldiv@1.0.1
	nalgebra-macros@0.3.0
	nalgebra@0.35.0
	native-tls@0.2.18
	ndarray@0.17.2
	nix@0.29.0
	nom@7.1.3
	nu-ansi-term@0.50.3
	num_threads@0.1.7
	num-bigint@0.4.8
	num-complex@0.4.6
	num-conv@0.2.2
	num-derive@0.4.2
	num-integer@0.1.47
	num-rational@0.4.2
	num-traits@0.2.19
	oid@0.2.1
	once_cell_polyfill@1.70.2
	once_cell@1.21.4
	opencv-binding-generator@0.105.0
	opencv@0.100.1
	openssl-macros@0.1.1
	openssl-probe@0.2.1
	openssl-sys@0.9.117
	openssl@0.10.81
	option-operations@0.6.1
	ordered-float@4.6.0
	ordered-stream@0.2.0
	ort-sys@2.0.0-rc.13
	ort@2.0.0-rc.13
	palette_derive@0.7.7
	palette_math@0.7.7
	palette@0.7.7
	pango-sys@0.22.9
	pango@0.22.9
	parking_lot_core@0.9.12
	parking_lot@0.12.5
	parking@2.2.1
	pastey@0.2.3
	pbkdf2@0.13.0
	pem-rfc7468@1.0.0
	percent-encoding@2.3.2
	pest_derive@2.9.1
	pest_generator@2.9.1
	pest_meta@2.9.1
	pest@2.9.1
	phf_codegen@0.11.3
	phf_generator@0.11.3
	phf_macros@0.11.3
	phf_shared@0.11.3
	phf@0.11.3
	picky-asn1-der@0.5.6
	picky-asn1-x509@0.15.4
	picky-asn1@0.10.1
	pin-project-lite@0.2.17
	piper@0.2.5
	pkg-config@0.3.34
	polling@3.11.0
	polyval@0.7.3
	portable-atomic-util@0.2.8
	portable-atomic@1.15.0
	powerfmt@0.2.0
	ppmd-rust@1.4.1
	proc-macro-crate@3.5.0
	proc-macro2@1.0.107
	pxfm@0.1.30
	quote@1.0.47
	r-efi@5.3.0
	r-efi@6.0.0
	rand_core@0.10.1
	rand_core@0.6.4
	rand@0.8.8
	ratatui-core@0.1.2
	ratatui-crossterm@0.1.2
	ratatui-macros@0.7.2
	ratatui-termina@0.1.0
	ratatui-termwiz@0.1.2
	ratatui-widgets@0.3.2
	ratatui@0.30.2
	rawpointer@0.2.1
	redox_syscall@0.5.18
	regex-automata@0.4.18
	regex-syntax@0.8.11
	regex@1.13.1
	ring@0.17.14
	rustc_version@0.4.1
	rustix@1.1.4
	rustls-pki-types@1.15.1
	rustls-webpki@0.103.15
	rustls@0.23.43
	rustversion@1.0.23
	ryu@1.0.23
	safe_arch@1.2.0
	schannel@0.1.29
	scopeguard@1.2.0
	security-framework-sys@2.17.0
	security-framework@3.7.0
	semver@1.0.28
	serde_bytes@0.11.19
	serde_core@1.0.229
	serde_derive@1.0.229
	serde_plain@1.0.2
	serde_repr@0.1.21
	serde_spanned@1.1.1
	serde@1.0.229
	sha1@0.11.0
	sha2@0.10.9
	sha2@0.11.0
	sharded-slab@0.1.7
	shell-words@1.1.1
	shlex@2.0.1
	signal-hook-mio@0.2.5
	signal-hook-registry@1.4.8
	signal-hook@0.3.18
	simba@0.10.2
	simd-adler32@0.3.10
	siphasher@1.0.3
	slab@0.4.12
	smallvec@1.16.0
	socket2@0.6.5
	socks@0.3.4
	stable_deref_trait@1.2.1
	static_assertions@1.1.0
	strsim@0.11.1
	strum_macros@0.28.0
	strum@0.28.0
	subtle@2.6.1
	syn@1.0.109
	syn@2.0.119
	syn@3.0.5
	system-deps@7.0.8
	system-deps@9.0.0
	target-lexicon@0.12.16
	target-lexicon@0.13.5
	tempfile@3.27.0
	termina@0.3.3
	terminfo@0.9.0
	termios@0.3.3
	termwiz@0.23.3
	thiserror-impl@1.0.69
	thiserror-impl@2.0.20
	thiserror@1.0.69
	thiserror@2.0.20
	thread_local@1.1.10
	time-core@0.1.9
	time@0.3.55
	tokio-macros@2.7.2
	tokio@1.53.1
	toml_datetime@1.1.1+spec-1.1.0
	toml_edit@0.25.13+spec-1.1.0
	toml_parser@1.1.3+spec-1.1.0
	toml_writer@1.1.2+spec-1.1.0
	toml@1.1.5+spec-1.1.0
	tracing-attributes@0.1.31
	tracing-core@0.1.36
	tracing-log@0.2.0
	tracing-subscriber@0.3.23
	tracing@0.1.44
	tss-esapi-sys@0.6.0
	tss-esapi@7.7.0
	typed-path@0.12.3
	typenum@1.20.1
	ucd-trie@0.1.7
	uds_windows@1.2.1
	unicode-ident@1.0.24
	unicode-segmentation@1.13.3
	unicode-truncate@2.0.1
	unicode-width@0.2.2
	universal-hash@0.6.1
	untrusted@0.9.0
	ureq-proto@0.6.2
	ureq@3.4.1
	utf8-zero@0.8.1
	utf8parse@0.2.2
	uuid@1.26.0
	valuable@0.1.1
	vcpkg@0.2.15
	version_check@0.9.5
	version-compare@0.2.1
	vtparse@0.6.2
	wasi@0.11.1+wasi-snapshot-preview1
	wasip2@1.0.4+wasi-0.2.12
	wasm-bindgen-macro-support@0.2.128
	wasm-bindgen-macro@0.2.128
	wasm-bindgen-shared@0.2.128
	wasm-bindgen@0.2.128
	webpki-root-certs@1.0.9
	webpki-roots@1.0.9
	wezterm-bidi@0.2.3
	wezterm-blob-leases@0.1.1
	wezterm-color-types@0.3.0
	wezterm-dynamic-derive@0.1.1
	wezterm-dynamic@0.2.1
	wezterm-input-types@0.1.0
	wide@1.7.0
	winapi-i686-pc-windows-gnu@0.4.0
	winapi-x86_64-pc-windows-gnu@0.4.0
	winapi@0.3.9
	windows_aarch64_gnullvm@0.52.6
	windows_aarch64_msvc@0.52.6
	windows_i686_gnu@0.52.6
	windows_i686_gnullvm@0.52.6
	windows_i686_msvc@0.52.6
	windows_x86_64_gnu@0.52.6
	windows_x86_64_gnullvm@0.52.6
	windows_x86_64_msvc@0.52.6
	windows-collections@0.3.2
	windows-core@0.62.2
	windows-future@0.3.2
	windows-implement@0.60.2
	windows-interface@0.59.3
	windows-link@0.2.1
	windows-numerics@0.3.1
	windows-result@0.4.1
	windows-strings@0.5.1
	windows-sys@0.52.0
	windows-sys@0.61.2
	windows-targets@0.52.6
	windows-threading@0.2.1
	windows@0.62.2
	winnow@1.0.4
	winsplit@0.1.0
	wit-bindgen@0.57.1
	zbus_macros@5.19.0
	zbus_names@4.3.4
	zbus_polkit@5.1.0
	zbus@5.19.0
	zcheapstr@1.1.0
	zeroize_derive@1.5.0
	zeroize@1.9.0
	zip@8.6.0
	zlib-rs@0.6.7
	zopfli@0.8.3
	zstd-safe@7.3.0
	zstd-sys@2.1.0+zstd.1.5.7
	zstd@0.13.3
	zune-core@0.5.3
	zune-jpeg@0.5.15
	zvariant_derive@5.15.0
	zvariant_utils@4.2.0
	zvariant@5.15.0
"

RUST_MIN_VER="1.96.0"

inherit cargo desktop gnome2-utils systemd

DESCRIPTION="Facial authentication for Linux"
HOMEPAGE="
	https://gaze.gundulabs.com
	https://github.com/GunduLabs/gaze
"
SRC_URI="
	https://github.com/GunduLabs/gaze/archive/refs/tags/v${PV}.tar.gz -> ${P}.tar.gz
	${CARGO_CRATE_URIS}
"

S="${WORKDIR}/${P}"

LICENSE="GPL-3+"
# Dependent crate licenses
LICENSE+=" Apache-2.0 BSD ISC MIT MPL-2.0 Unicode-3.0 ZLIB"
LICENSE+=" || ( Apache-2.0 MIT )"
SLOT="0"
KEYWORDS="~amd64"
IUSE="gnome gui"

RDEPEND="
	app-crypt/tpm2-tss:=
	dev-libs/openssl:=
	media-libs/gst-plugins-base:1.0
	media-libs/gst-plugins-good:1.0
	media-plugins/gst-plugins-jpeg
	media-plugins/gst-plugins-v4l2
	media-libs/gstreamer:1.0
	media-libs/libv4l
	media-video/pipewire[gstreamer]
	media-libs/opencv:=
	sci-libs/onnxruntime-bin:=
	sys-libs/pam
	gui? (
		gui-libs/gtk:4
		gui-libs/libadwaita:1
	)
	gnome? ( gnome-base/gnome-shell )
"
DEPEND="${RDEPEND}"
BDEPEND="
	llvm-core/clang
	virtual/pkgconfig
"

RESTRICT="test"

QA_FLAGS_IGNORED="
	usr/bin/gazed
	usr/bin/gaze
	usr/bin/gaze-gui
	usr/lib*/security/pam_gaze.so
	usr/lib*/security/pam_gaze_grosshack.so
"

src_prepare() {
	default
	if ! use gnome; then
		sed -i 's| /etc/dconf/db$||' packaging/config/gazed.service || die
	fi
}

src_configure() {
	cargo_src_configure
}

src_compile() {
	export ORT_LIB_LOCATION="${ESYSROOT}/usr/$(get_libdir)"
	export ORT_PREFER_DYNAMIC_LINK=1

	# Daemon must be built separately from client binaries. The `gaze` crate
	# enables gaze-core's `detection` feature (which pulls in ort/ONNX Runtime).
	# Client binaries use gaze-core with default-features=false to avoid linking
	# ONNX Runtime, whose static constructors require AVX2 and would crash on
	# older CPUs.
	local target_args=( -p gaze )
	set -- "${CARGO}" build $(usex debug "" --release) ${ECARGO_ARGS[@]} "${target_args[@]}"
	einfo "${@}"
	cargo_env "${@}" || die "daemon build failed"

	target_args=( -p gaze-cli -p pam-gaze -p pam-gaze-grosshack )
	if use gui; then
		target_args+=( -p gaze-gui )
	fi
	set -- "${CARGO}" build $(usex debug "" --release) ${ECARGO_ARGS[@]} "${target_args[@]}"
	einfo "${@}"
	cargo_env "${@}" || die "client build failed"
}

src_install() {
	local target_dir
	target_dir=$(cargo_target_dir)

	dobin "${target_dir}/gazed"
	dobin "${target_dir}/gaze"
	use gui && dobin "${target_dir}/gaze-gui"

	exeinto "/usr/$(get_libdir)/security"
	newexe "${target_dir}/libpam_gaze.so" pam_gaze.so
	newexe "${target_dir}/libpam_gaze_grosshack.so" pam_gaze_grosshack.so

	insinto /etc/gaze
	doins packaging/config/config.toml

	insinto /etc/dbus-1/system.d
	doins packaging/config/com.gundulabs.Gaze.conf

	insinto /usr/share/polkit-1/actions
	doins packaging/config/com.gundulabs.gaze.policy

	systemd_dounit packaging/config/gazed.service

	if use gui; then
		domenu packaging/gui/com.gundulabs.Gaze.desktop
		doicon -s scalable packaging/gui/com.gundulabs.Gaze.svg
		insinto /usr/share/metainfo
		doins packaging/gui/com.gundulabs.Gaze.metainfo.xml
	fi

	if use gnome; then
		local extdir="/usr/share/gnome-shell/extensions/gaze@gundulabs.com"
		insinto "${extdir}"
		doins gnome-shell-extension/{metadata.json,extension.js,prefs.js}

		insinto /usr/share/glib-2.0/schemas
		doins packaging/config/org.gnome.shell.extensions.gaze.gschema.xml
	fi
}

pkg_postinst() {
	use gui && xdg_desktop_database_update && xdg_icon_cache_update
	use gnome && gnome2_schemas_update

	elog "The gazed daemon downloads ML models on first use."
	elog "To start: systemctl enable --now gazed"
	elog "To enroll: gaze enroll"
	elog ""
	elog "For TPM-sealed template encryption, set encrypt_templates = true"
	elog "in /etc/gaze/config.toml (requires a TPM 2.0)."
	elog ""
	elog "To enable face auth, add pam_gaze.so to your PAM stack."
	elog "Example /etc/pam.d entry for a lock screen:"
	elog "  auth [success=done default=ignore] pam_gaze.so"
	elog "See https://gaze.gundulabs.com for PAM integration guides."
}

pkg_postrm() {
	use gui && xdg_desktop_database_update && xdg_icon_cache_update
	use gnome && gnome2_schemas_update
}

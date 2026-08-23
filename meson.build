project('ous', 'rust',
  version : '0.7.0',
  license : 'MIT',
  default_options : [
    'optimization=3',
    'strip=true',
  ],
)

# B2: profile is threaded through to cargo (-Dprofile=debug builds unoptimized).
profile = get_option('profile')
if profile == 'debug'
  cargo_profile = 'dev'
  cargo_dir = 'debug'
else
  cargo_profile = profile
  cargo_dir = profile
endif

cpu = host_machine.cpu_family()
if cpu == 'x86_64'
  rust_target = 'x86_64-unknown-linux-musl'
elif cpu == 'aarch64'
  rust_target = 'aarch64-unknown-linux-musl'
else
  error('Unsupported architecture: ' + cpu + '. Supported: x86_64, aarch64')
endif

build_dir = meson.project_build_root() / 'cargo'
src_dir = meson.project_source_root()

custom_target('ous',
  build_by_default : true,
  build_always_stale : true,
  install : true,
  install_dir : get_option('prefix') / 'bin',
  command : ['sh', '-c',
    'CARGO_TARGET_DIR={0} cargo build --{1} --locked --manifest-path {2}/Cargo.toml --target {3} && cp {0}/{3}/{4}/ous @OUTPUT@'
    .format(build_dir, cargo_profile, src_dir, rust_target, cargo_dir),
  ],
  output : 'ous',
)

install_data(src_dir / 'docs' / 'ous.1',
  install_dir : get_option('prefix') / 'share' / 'man' / 'man1')

# `cps` is an external package: clone the upstream repo and build the cps
# binary from that clone (host target — cps is a host-side Python subsystem
# tool). The literal below MUST stay in sync with CPS_REV in env.mk and
# Cargo.toml; the CPS_REV environment variable overrides it.
cps_src = meson.project_build_root() / 'cps'
cps_ref = 'c4ba21e185398558052acec3f0b4619b4e8c0678'

custom_target('cps',
  build_by_default : false,
  build_always_stale : true,
  install : true,
  install_dir : get_option('prefix') / 'bin',
  command : ['sh', '-c',
    'd={0} && (test -d "$d/.git" || git clone --quiet https://github.com/Mapuse/CPS "$d") && git -C "$d" checkout --quiet "${{CPS_REV:-{1}}}" && CARGO_TARGET_DIR={2}/cargo-cps cargo build --release --locked --manifest-path "$d/Cargo.toml" && cp {2}/cargo-cps/release/cps @OUTPUT@ && mkdir -p {2}/cps-data/themes {2}/cps-data/examples && (test ! -d "$d/themes" || cp "$d"/themes/*.py {2}/cps-data/themes/) && (test ! -f "$d/t.desc" || cp "$d/t.desc" "$d/p.desc" {2}/cps-data/) && (test ! -d "$d/examples" || cp "$d"/examples/*.py {2}/cps-data/examples/) && cp -r {3}/themes/*.py {2}/cps-data/themes/ 2>/dev/null || true'
    .format(cps_src, cps_ref, meson.project_build_root(), src_dir),
  ],
  output : 'cps',
)

# Stage and install cps data files produced by the clone step above.
cps_data = meson.project_build_root() / 'cps-data'
meson.add_install_script('sh', '-c',
  'test -d "{0}" || exit 0; sd="$MESON_INSTALL_DESTDIR_PREFIX/share/cps"; mkdir -p "$sd/themes" "$sd/examples"; cp "{0}"/t.desc "{0}"/p.desc "$sd/" 2>/dev/null || true; cp "{0}"/themes/*.py "$sd/themes/" 2>/dev/null || true; cp "{0}"/examples/*.py "$sd/examples/" 2>/dev/null || true'
  .format(cps_data))

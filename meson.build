project('ous', 'rust',
  version : '0.7.0',
  license : 'unlicense',
  default_options : [
    'optimization=3',
    'strip=true',
  ],
)

gen_cross = find_program('gen-cross.sh')
run_command(gen_cross, check : true)

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
    'CARGO_TARGET_DIR={0} cargo build --release --locked --manifest-path {1}/Cargo.toml --target {2} && cp {0}/{2}/release/ous @OUTPUT@'
    .format(build_dir, src_dir, rust_target),
  ],
  output : 'ous',
)

# `cps` is an external package: clone the upstream repo and build the cps
# binary from that clone (host target — cps is a host-side Python subsystem
# tool). Its themes/data are installed by the Makefile's `install-cps`.
cps_src = meson.project_build_root() / 'cps'
cps_ref = 'd8d5f7da49917ea7636147b8a65f3541311c45ab'

custom_target('cps',
  build_by_default : true,
  build_always_stale : true,
  install : true,
  install_dir : get_option('prefix') / 'bin',
  command : ['sh', '-c',
    'd={0} && (test -d "$d/.git" || git clone --quiet https://github.com/Mapuse/CPS "$d") && git -C "$d" checkout --quiet {1} && CARGO_TARGET_DIR={2}/cargo-cps cargo build --release --locked --manifest-path "$d/Cargo.toml" && cp {2}/cargo-cps/release/cps @OUTPUT@'
    .format(cps_src, cps_ref, meson.project_build_root()),
  ],
  output : 'cps',
)

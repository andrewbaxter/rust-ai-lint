# The checker is compiled against the compiler's own libraries and then loaded
# into cargo in place of rustc, so the compiler it links against and the compiler
# it wraps have to be the same one. Nix is what makes that reasonable: everything
# here is pinned to the rustc in this nixpkgs.
{ pkgs ? import <nixpkgs> { } }:
let
  # The sysroot holding librustc_driver and the rustc_* rlibs. The wrapper in
  # `pkgs.rustc` is the compiler to run; this is where its libraries live.
  compiler = pkgs.rustc.unwrapped;
  # librustc_driver is linked against LLVM, which is packaged separately.
  llvm = compiler.llvm.lib;
in
pkgs.rustPlatform.buildRustPackage {
  pname = "rust-ai-lint";
  version = "0.2.0";
  src = ./.;
  cargoLock.lockFile = ./Cargo.lock;

  # Reaching into the compiler is a nightly-only thing to do. Release builds of
  # rustc carry the same libraries and this is the switch that opens them; the
  # version can't drift out from under us because nixpkgs pins it.
  RUSTC_BOOTSTRAP = 1;
  RUSTFLAGS = "-L native=${llvm}/lib";

  nativeBuildInputs = [ pkgs.makeWrapper ];

  postInstall = ''
    wrapProgram $out/bin/rust-ai-lint \
      --prefix LD_LIBRARY_PATH : "${compiler}/lib:${llvm}/lib" \
      --prefix PATH : "${pkgs.lib.makeBinPath [ pkgs.cargo pkgs.rustc ]}"
  '';

  doCheck = false;

  meta = {
    description = "Convention checker that runs inside the rust compiler";
    mainProgram = "rust-ai-lint";
  };
}

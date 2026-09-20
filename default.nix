{ pkgs ? import <nixpkgs> { } }:
let
  compiler = pkgs.rustc.unwrapped;
  llvm = compiler.llvm.lib;
in
pkgs.rustPlatform.buildRustPackage {
  pname = "rust-ai-lint";
  version = "0.2.0";
  src = ./.;
  cargoLock.lockFile = ./Cargo.lock;

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

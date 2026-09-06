{ pkgs ? import <nixpkgs> { } }:
pkgs.rustPlatform.buildRustPackage {
  pname = "schemask-lint";
  version = "0.1.0";
  src = ./.;
  cargoLock.lockFile = ./Cargo.lock;
  nativeBuildInputs = [ pkgs.makeWrapper ];
  postInstall = ''
    wrapProgram $out/bin/schemask-lint \
      --set-default RUST_SRC_PATH "${pkgs.rustPlatform.rustLibSrc}"
  '';
  doCheck = false;
  meta.description = "Convention linter for the schemask repo";
}

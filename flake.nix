{
  description = "hyprwhspr-rs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
  }:
    flake-utils.lib.eachSystem [
      "x86_64-linux"
      "aarch64-linux"
    ] (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

        hyprwhspr-rs = pkgs.rustPlatform.buildRustPackage {
          pname = cargoToml.package.name;
          version = cargoToml.package.version;

          src = self;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = with pkgs; [
            makeWrapper
            pkg-config
          ];

          buildInputs = with pkgs; [
            alsa-lib
            libxkbcommon
            onnxruntime
            openssl
            systemdMinimal
          ];

          env = {
            ORT_LIB_LOCATION = "${pkgs.lib.getLib pkgs.onnxruntime}/lib";
            ORT_PREFER_DYNAMIC_LINK = "1";
            ORT_STRATEGY = "system";
          };

          postInstall = ''
            install -Dm644 assets/* -t "$out/share/hyprwhspr-rs/assets"
            wrapProgram "$out/bin/hyprwhspr-rs" \
              --prefix PATH : "${pkgs.lib.makeBinPath [ pkgs.whisper-cpp ]}"
          '';

          meta = {
            description = "Native speech-to-text voice dictation for Hyprland";
            homepage = "https://github.com/better-slop/hyprwhspr-rs";
            license = pkgs.lib.licenses.mit;
            mainProgram = "hyprwhspr-rs";
            platforms = pkgs.lib.platforms.linux;
          };
        };
      in
      {
        packages.default = hyprwhspr-rs;
        checks.default = hyprwhspr-rs;
        apps.default = {
          type = "app";
          program = "${hyprwhspr-rs}/bin/hyprwhspr-rs";
          meta.description = "Run hyprwhspr-rs";
        };
      }
    );
}

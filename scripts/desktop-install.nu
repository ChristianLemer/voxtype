#!/usr/bin/env nu
# Install a desktop build of this fork on Linux, in front of the
# distribution's voxtype.
#
# The binary goes to /usr/local/bin/voxtype, which comes before /usr/bin in
# PATH and which pacman, apt and dnf never touch, so a distribution update
# does not overwrite it. The distribution's package stays installed and
# keeps providing the OSD, the audio bridge and the Quickshell files. A
# systemd drop-in points the user service at the new binary.
#
# Dry run by default: it says what it would do. Pass --force to do it.
#
#   nu scripts/desktop-install.nu                     # latest desktop build
#   nu scripts/desktop-install.nu --tag desktop-1.1.1-15 --force
#   nu scripts/desktop-install.nu --remove --force    # back to the distribution's
#
# Needs gh (authenticated), sha256sum and sudo.

const REPO = "ChristianLemer/voxtype"
const TARGET = "/usr/local/bin/voxtype"
const DROPIN_NAME = "desktop.conf"

def dropin-path [] {
    $env.HOME | path join ".config/systemd/user/voxtype.service.d" $DROPIN_NAME
}

def say [msg: string] { print $"(ansi cyan)voxtype-desktop(ansi reset) ($msg)" }

def need [cmd: string] {
    if (which $cmd | is-empty) {
        error make {msg: $"($cmd) is required and was not found in PATH"}
    }
}

# The latest desktop-* release, or the one asked for.
def resolve-tag [tag?: string] {
    if $tag != null { return $tag }
    let tags = (^gh release list -R $REPO --limit 30 --json tagName
        | from json | get tagName | where {|t| $t starts-with "desktop-"})
    if ($tags | is-empty) { error make {msg: $"no desktop-* release found in ($REPO)"} }
    $tags | first
}

def restart-service [] {
    ^systemctl --user daemon-reload
    ^systemctl --user restart voxtype
    sleep 2sec
    let state = (^systemctl --user is-active voxtype | complete | get stdout | str trim)
    say $"service: ($state)"
}

def main [
    --tag: string   # a desktop-* tag; the latest desktop build by default
    --force         # do it; without it, only say what would happen
    --remove        # remove the desktop build and go back to the distribution's
] {
    if $remove {
        say $"would remove ($TARGET) and (dropin-path)"
        if not $force { say "dry run: pass --force to do it"; return }
        ^sudo rm -f $TARGET
        rm -f (dropin-path)
        restart-service
        say $"back to (^/usr/bin/voxtype --version | str trim)"
        return
    }

    need gh
    need sha256sum
    let tag = (resolve-tag $tag)
    say $"release: ($tag)"

    let dir = (mktemp -d --tmpdir voxtype-desktop.XXXXXX)
    ^gh release download $tag -R $REPO --dir $dir -p "voxtype-*-linux-x86_64" -p "SHA256SUMS.txt"
    let bin = (ls $dir | where name =~ 'linux-x86_64$' | get name | first)
    let file = ($bin | path basename)

    # Only the Linux line of the checksums: the other files are not downloaded.
    let sums = (open --raw ($dir | path join "SHA256SUMS.txt") | lines | where {|l| $l ends-with $file})
    if ($sums | is-empty) { error make {msg: $"($file) is not in SHA256SUMS.txt"} }
    let check = (do { cd $dir; $sums | str join "\n" | ^sha256sum -c - | complete })
    if $check.exit_code != 0 { error make {msg: $"checksum mismatch for ($file)"} }
    say $"checksum: OK"

    ^chmod +x $bin
    let version = (^$bin --version | str trim)
    say $"binary: ($version)"

    let dropin = $"[Service]\nExecStart=\nExecStart=($TARGET) daemon\n"
    say $"would install it as ($TARGET) and point the service at it \((dropin-path)\)"
    if not $force {
        say "dry run: pass --force to do it"
        rm -rf $dir
        return
    }

    ^sudo install -m 755 $bin $TARGET
    mkdir (dropin-path | path dirname)
    $dropin | save --force (dropin-path)
    rm -rf $dir
    restart-service
    say $"installed: (^$TARGET --version | str trim) \(($tag)\)"
}

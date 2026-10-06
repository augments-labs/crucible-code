# The installer receipt, read the way `crucible-update` reads it.
#
# A receipt is the last file of a release unit,
# `<dir>/.crucible-install/releases/<version>/receipt`. It names the unit's
# installation, platform, prefix and release, and the SHA-256 of each
# executable the unit holds. The format is line text, version 1:
#
#     crucible-installer-receipt 1
#     manager=crucible-installer
#     installation=<32 lowercase hex digits, fixed for the install's life>
#     target=<linux|macos|freebsd>-<x86_64|aarch64>
#     layout=versioned
#     prefix=<the absolute, canonical prefix>
#     version=<major>.<minor>.<patch>
#     sha256.crucible=<64 lowercase hex digits>
#     sha256.crucible-sandbox-broker=<64 lowercase hex digits>
#
# Every key appears exactly once, in that order, and each line ends in a
# newline. Only the broker line may be left out, when the release carries no
# broker. Anything else is refused: an unknown, repeated or reordered key, a
# control character, a relative prefix or one with `.` or `..` in it, and more
# than 8192 bytes. A receipt whose first line names a later format version was
# written by a newer installer and is refused as one. A flat install has no
# receipt.
#
# The Rust parser and this function are held to the same answer on every
# receipt in `receipts/`. Call it under `LC_ALL=C`, so that a byte above 127
# in a prefix is one byte to every pattern below.
#
# On success it returns 0 and sets `receipt_installation`, `receipt_target`,
# `receipt_prefix`, `receipt_version`, `receipt_crucible` and `receipt_broker`
# (empty when the release carries no broker). On refusal it says why on
# standard error and returns 1.

crucible_receipt_read() {
    receipt_file=$1
    receipt_installation=
    receipt_target=
    receipt_prefix=
    receipt_version=
    receipt_crucible=
    receipt_broker=
    receipt_line=0

    receipt_size=$(wc -c <"$receipt_file") || return 1
    if [ "$((receipt_size))" -gt 8192 ]; then
        crucible_receipt_refuse 'the receipt is larger than 8192 bytes'
        return 1
    fi
    receipt_controls=$(LC_ALL=C tr -d '\n\040-\176\200-\377' <"$receipt_file" | wc -c) ||
        return 1
    if [ "$((receipt_controls))" -ne 0 ]; then
        crucible_receipt_refuse 'the receipt holds a control character'
        return 1
    fi
    if [ "$((receipt_size))" -gt 0 ] && [ -n "$(tail -c 1 "$receipt_file")" ]; then
        crucible_receipt_refuse 'the last line of the receipt does not end'
        return 1
    fi

    while IFS= read -r receipt_text; do
        receipt_line=$((receipt_line + 1))
        case $receipt_line in
        1)
            case $receipt_text in
            'crucible-installer-receipt 1') ;;
            'crucible-installer-receipt '[1-9]*)
                case ${receipt_text#crucible-installer-receipt } in
                *[!0-9]*)
                    crucible_receipt_refuse 'this is not an installer receipt'
                    return 1
                    ;;
                esac
                crucible_receipt_refuse 'the receipt was written by a newer installer'
                return 1
                ;;
            *)
                crucible_receipt_refuse 'this is not an installer receipt'
                return 1
                ;;
            esac
            ;;
        2)
            crucible_receipt_value manager || return 1
            if [ "$receipt_value" != crucible-installer ]; then
                crucible_receipt_refuse 'manager is not crucible-installer'
                return 1
            fi
            ;;
        3)
            crucible_receipt_value installation || return 1
            crucible_receipt_hex 32 installation || return 1
            receipt_installation=$receipt_value
            ;;
        4)
            crucible_receipt_value target || return 1
            case $receipt_value in
            linux-x86_64 | linux-aarch64 | macos-x86_64 | macos-aarch64 | freebsd-x86_64) ;;
            *)
                crucible_receipt_refuse 'target names no platform the installer supports'
                return 1
                ;;
            esac
            receipt_target=$receipt_value
            ;;
        5)
            crucible_receipt_value layout || return 1
            if [ "$receipt_value" != versioned ]; then
                crucible_receipt_refuse 'layout is not versioned'
                return 1
            fi
            ;;
        6)
            crucible_receipt_value prefix || return 1
            case $receipt_value in
            /*) ;;
            *)
                crucible_receipt_refuse 'prefix is not an absolute path'
                return 1
                ;;
            esac
            case $receipt_value in
            */ | *//* | */./* | */. | */../* | */..)
                crucible_receipt_refuse 'prefix is not a canonical path'
                return 1
                ;;
            esac
            receipt_prefix=$receipt_value
            ;;
        7)
            crucible_receipt_value version || return 1
            receipt_rest=${receipt_value#*.}
            case $receipt_value in
            *.*) ;;
            *) receipt_rest= ;;
            esac
            case $receipt_rest in
            *.*) ;;
            *) receipt_rest= ;;
            esac
            if [ -z "$receipt_rest" ] ||
                ! crucible_receipt_number "${receipt_value%%.*}" ||
                ! crucible_receipt_number "${receipt_rest%%.*}" ||
                ! crucible_receipt_number "${receipt_rest#*.}"; then
                crucible_receipt_refuse 'version is not a release number'
                return 1
            fi
            receipt_version=$receipt_value
            ;;
        8)
            crucible_receipt_value sha256.crucible || return 1
            crucible_receipt_hex 64 sha256.crucible || return 1
            receipt_crucible=$receipt_value
            ;;
        9)
            crucible_receipt_value sha256.crucible-sandbox-broker || return 1
            crucible_receipt_hex 64 sha256.crucible-sandbox-broker || return 1
            receipt_broker=$receipt_value
            ;;
        *)
            crucible_receipt_refuse 'nothing may follow the last key'
            return 1
            ;;
        esac
    done <"$receipt_file"

    if [ "$receipt_line" -lt 8 ]; then
        crucible_receipt_refuse 'the receipt ends before its last key'
        return 1
    fi
}

# Takes the value of line `receipt_text` into `receipt_value` when the line
# names key `$1`.
crucible_receipt_value() {
    case $receipt_text in
    "$1="*) receipt_value=${receipt_text#"$1="} ;;
    *)
        crucible_receipt_refuse "line $receipt_line is not $1"
        return 1
        ;;
    esac
}

# Whether `receipt_value` is exactly `$1` lowercase hex digits.
crucible_receipt_hex() {
    case $receipt_value in
    '' | *[!0-9a-f]*)
        crucible_receipt_refuse "$2 is not lowercase hex"
        return 1
        ;;
    esac
    if [ "${#receipt_value}" -ne "$1" ]; then
        crucible_receipt_refuse "$2 is not $1 hex digits"
        return 1
    fi
}

# Whether `$1` is a decimal number written without a leading zero.
crucible_receipt_number() {
    case $1 in
    '' | 0?* | *[!0-9]*) return 1 ;;
    esac
}

crucible_receipt_refuse() {
    if [ "$receipt_line" -gt 0 ]; then
        printf 'refused: line %s: %s\n' "$receipt_line" "$1" >&2
    else
        printf 'refused: %s\n' "$1" >&2
    fi
}

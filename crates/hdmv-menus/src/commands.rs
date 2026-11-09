//! HDMV navigation commands: 12 bytes each, in movie objects and on menu buttons. Decoded here into
//! plain data; run by [`crate::vm`].

use serde::Serialize;

/// An operand: a value given in the command, or a register (general purpose or player status).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Operand {
    Value(u32),
    Gpr(u32),
    Psr(u32),
}

impl Operand {
    /// A set or compare command's operand: a value, or a register (the top bit: a player status one).
    fn new(value: u32, immediate: bool) -> Self {
        if immediate {
            Operand::Value(value)
        } else if value & 0x8000_0000 != 0 {
            Operand::Psr(value & 0x7F)
        } else {
            Operand::Gpr(value & 0xFFF)
        }
    }

    /// A system command's operand (SetButtonPage, EnableButton...): its top bits are flags (whether the
    /// operand is given at all), the rest a 16-bit value or a general register's number.
    fn system(value: u32, immediate: bool) -> Self {
        if immediate {
            Operand::Value(value & 0xFFFF)
        } else {
            Operand::Gpr(value & 0xFFF)
        }
    }
}

impl std::fmt::Display for Operand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Operand::Value(v) => write!(f, "{v}"),
            Operand::Gpr(r) => write!(f, "r{r}"),
            Operand::Psr(r) => write!(f, "psr{r}"),
        }
    }
}

/// One command: its operation's name ("PlayPL", "set_move", "cmp_eq", "SetButtonPage"...), its two
/// operands, and its 12 bytes as hex. A system command also says which of its operands are given
/// (`given`: SetButtonPage may set the button, the page, or both).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Command {
    pub op: String,
    pub dst: Operand,
    pub src: Operand,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub given: Option<[bool; 2]>,
    pub raw: String,
    /// The 12 bytes themselves.
    #[serde(skip)]
    pub code: [u8; 12],
}

fn branch(sub: u8, op: u8) -> Option<&'static str> {
    Some(match (sub, op) {
        (0, 0) => "Nop",
        (0, 1) => "GoTo",
        (0, 2) => "Break",
        (1, 0) => "JumpObject",
        (1, 1) => "JumpTitle",
        (1, 2) => "CallObject",
        (1, 3) => "CallTitle",
        (1, 4) => "Resume",
        (2, 0) => "PlayPL",
        (2, 1) => "PlayPLatPI",
        (2, 2) => "PlayPLatMK",
        (2, 3) => "TerminatePL",
        (2, 4) => "LinkPI",
        (2, 5) => "LinkMK",
        _ => return None,
    })
}

const COMPARE: [&str; 8] = ["", "bc", "eq", "ne", "ge", "gt", "le", "lt"];
const SET: [&str; 16] =
    ["", "move", "swap", "add", "sub", "mul", "div", "mod", "rnd", "and", "or", "xor", "bitset", "bitclr", "shl", "shr"];
const SYSTEM: [&str; 10] = [
    "",
    "SetStream",
    "SetNVTimer",
    "SetButtonPage",
    "EnableButton",
    "DisableButton",
    "SetSecondaryStream",
    "PopUpMenuOff",
    "StillOn",
    "StillOff",
];

/// Decode one command's 12 bytes.
pub fn decode(c: &[u8; 12]) -> Command {
    let group = (c[0] >> 3) & 3;
    let sub = c[0] & 7;
    let (imm1, imm2) = (c[1] & 0x80 != 0, c[1] & 0x40 != 0);
    let (branch_op, cmp_op, set_op) = (c[1] & 15, c[2] & 15, c[3] & 31);
    let dst = u32::from_be_bytes([c[4], c[5], c[6], c[7]]);
    let src = u32::from_be_bytes([c[8], c[9], c[10], c[11]]);
    let system = group == 2 && sub == 1;
    let op = match group {
        0 => branch(sub, branch_op).map(String::from).unwrap_or_else(|| format!("branch{sub}.{branch_op}")),
        1 => match COMPARE.get(cmp_op as usize).filter(|s| !s.is_empty()) {
            Some(s) => format!("cmp_{s}"),
            None => format!("cmp_{cmp_op}"),
        },
        _ if sub == 0 => match SET.get(set_op as usize).filter(|s| !s.is_empty()) {
            Some(s) => format!("set_{s}"),
            None => format!("set_{set_op}"),
        },
        _ if system => SYSTEM
            .get(set_op as usize)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("system{set_op}")),
        _ => format!("set{sub}.{set_op}"),
    };
    let (dst_op, src_op, given) = if system {
        (Operand::system(dst, imm1), Operand::system(src, imm2), Some([dst & 0x8000_0000 != 0, src & 0x8000_0000 != 0]))
    } else {
        (Operand::new(dst, imm1), Operand::new(src, imm2), None)
    };
    Command { op, dst: dst_op, src: src_op, given, raw: c.iter().map(|b| format!("{b:02x}")).collect(), code: *c }
}

impl std::fmt::Display for Command {
    /// One line: "set_move r4076, 31", "CallTitle r4076", "SetButtonPage button r4076, page r4077".
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let op = self.op.as_str();
        if matches!(op, "Nop" | "Break" | "Resume" | "TerminatePL" | "PopUpMenuOff" | "StillOn" | "StillOff") {
            write!(f, "{op}")
        } else if op == "SetButtonPage" {
            let [button, page] = self.given.unwrap_or([true, true]);
            let show = |given: bool, o: &Operand| if given { o.to_string() } else { "-".to_string() };
            write!(f, "{op} button {}, page {}", show(button, &self.dst), show(page, &self.src))
        } else if op.starts_with("set_") || op.starts_with("cmp_") || matches!(op, "SetStream" | "SetSecondaryStream") {
            write!(f, "{op} {}, {}", self.dst, self.src)
        } else {
            write!(f, "{op} {}", self.dst)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn cmd(head: [u8; 4], dst: u32, src: u32) -> [u8; 12] {
        let mut c = [0u8; 12];
        c[..4].copy_from_slice(&head);
        c[4..8].copy_from_slice(&dst.to_be_bytes());
        c[8..].copy_from_slice(&src.to_be_bytes());
        c
    }

    #[test]
    fn a_command_that_sets_a_register_and_one_that_calls_the_title_in_it() {
        let s = decode(&cmd([0x50, 0x40, 0, 1], 4076, 31));
        let c = decode(&cmd([0x21, 0x03, 0, 0], 4076, 0));
        assert_eq!((s.op.as_str(), s.dst, s.src), ("set_move", Operand::Gpr(4076), Operand::Value(31)));
        assert_eq!(s.to_string(), "set_move r4076, 31");
        assert_eq!(c.to_string(), "CallTitle r4076");
        assert_eq!(decode(&cmd([0x22, 0x80, 0, 0], 61, 0)).to_string(), "PlayPL 61");
    }

    #[test]
    fn a_system_commands_top_bit_says_its_operand_is_given_not_a_status_register() {
        // SetButtonPage: the button from r4076, the page from r4077, both given
        let both = decode(&cmd([0x51, 0x00, 0x00, 0x03], 0x8000_0FEC, 0x8000_0FED));
        assert_eq!((both.dst, both.src, both.given), (Operand::Gpr(4076), Operand::Gpr(4077), Some([true, true])));
        assert_eq!(both.to_string(), "SetButtonPage button r4076, page r4077");
        // the button only: the page stays
        let button = decode(&cmd([0x51, 0x00, 0x00, 0x03], 0x8000_0FEC, 0));
        assert_eq!(button.to_string(), "SetButtonPage button r4076, page -");
    }
}

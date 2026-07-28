use crate::id::{CanId, EXT_FLAG};

const ID_MASK: u32 = EXT_FLAG | 0x1fff_ffff;

/// 單一 CAN 識別碼過濾規則。
///
/// 基本比對公式為 `(id.to_bits() ^ self.id) & self.mask == 0`；反轉規則
/// 則由 [`FilterSet`] 在正向規則之後套用。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FilterRule {
    id: u32,
    mask: u32,
    invert: bool,
}

impl FilterRule {
    /// 建立精確比對單一識別碼與其標準／擴充格式的規則。
    #[must_use]
    pub const fn exact(id: CanId) -> Self {
        Self {
            id: id.to_bits(),
            mask: ID_MASK,
            invert: false,
        }
    }

    /// 建立自訂遮罩比對規則。
    ///
    /// 遮罩 bit 為一時要求該位元相同；若要區分標準與擴充格式，遮罩需包含
    /// [`EXT_FLAG`]。
    #[must_use]
    pub const fn mask(id: CanId, mask: u32) -> Self {
        Self {
            id: id.to_bits(),
            mask: mask & ID_MASK,
            invert: false,
        }
    }

    /// 將規則切換為反轉規則，符合者會被排除。
    #[must_use]
    pub const fn inverted(mut self) -> Self {
        self.invert = !self.invert;
        self
    }

    /// 判斷識別碼是否符合此規則本身。
    ///
    /// 此方法包含反轉語意；集合進行兩階段比對時會改用未反轉的基本結果。
    #[must_use]
    pub const fn matches(&self, id: CanId) -> bool {
        let base = ((id.to_bits() ^ self.id) & self.mask) == 0;
        if self.invert { !base } else { base }
    }

    const fn base_matches(&self, id: CanId) -> bool {
        ((id.to_bits() ^ self.id) & self.mask) == 0
    }
}

/// CAN 識別碼過濾器集合。
///
/// 空集合表示接受全部。集合只在設定或訂閱時用 `Vec` 配置一次，熱路徑的
/// [`matches`](Self::matches) 僅走訪唯讀切片，不進行任何配置。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FilterSet {
    rules: Vec<FilterRule>,
}

impl FilterSet {
    /// 建立接受所有識別碼的空集合。
    #[must_use]
    pub const fn accept_all() -> Self {
        Self { rules: Vec::new() }
    }

    /// 建立拒絕所有識別碼的集合。
    #[must_use]
    pub fn reject_all() -> Self {
        Self::with(FilterRule::mask(CanId::from_bits(0), 0).inverted())
    }

    /// 以單一規則建立集合。
    #[must_use]
    pub fn with(rule: FilterRule) -> Self {
        Self { rules: vec![rule] }
    }

    /// 在集合尾端加入規則，並回傳集合以便鏈式設定。
    pub fn push(&mut self, rule: FilterRule) -> &mut Self {
        self.rules.push(rule);
        self
    }

    /// 取得所有規則的唯讀切片。
    #[must_use]
    pub fn rules(&self) -> &[FilterRule] {
        &self.rules
    }

    /// 判斷集合是否為空的「接受全部」設定。
    #[must_use]
    pub fn is_accept_all(&self) -> bool {
        self.rules.is_empty()
    }

    /// 判斷識別碼是否通過集合。
    ///
    /// 所有非反轉規則先組成允許清單，任一符合即通過；沒有非反轉規則時
    /// 預設通過。接著套用反轉規則，任一基本比對符合就拒絕。
    #[must_use]
    pub fn matches(&self, id: CanId) -> bool {
        let mut has_positive = false;
        let mut positive_match = false;

        for rule in &self.rules {
            if !rule.invert {
                has_positive = true;
                positive_match |= rule.base_matches(id);
            }
        }
        if has_positive && !positive_match {
            return false;
        }

        !self
            .rules
            .iter()
            .any(|rule| rule.invert && rule.base_matches(id))
    }
}

#[cfg(test)]
mod tests {
    use super::{FilterRule, FilterSet};
    use crate::id::{CanId, EXT_FLAG};

    fn standard(raw: u16) -> CanId {
        CanId::standard(raw).unwrap_or_else(|error| unreachable!("{error}"))
    }

    #[test]
    fn covers_collection_semantics() {
        let a = standard(0x123);
        let b = standard(0x124);
        let extended = CanId::extended(0x123).unwrap_or_else(|error| unreachable!("{error}"));

        assert!(FilterSet::accept_all().matches(a));
        assert!(FilterSet::accept_all().is_accept_all());
        assert!(!FilterSet::reject_all().matches(a));

        let exact = FilterSet::with(FilterRule::exact(a));
        assert!(exact.matches(a));
        assert!(!exact.matches(b));
        assert!(!exact.matches(extended));

        let masked = FilterSet::with(FilterRule::mask(a, EXT_FLAG | 0x7f0));
        assert!(masked.matches(standard(0x12f)));
        assert!(!masked.matches(standard(0x130)));

        let exclusion_only = FilterSet::with(FilterRule::exact(a).inverted());
        assert!(!exclusion_only.matches(a));
        assert!(exclusion_only.matches(b));

        let mut combined = FilterSet::with(FilterRule::mask(a, EXT_FLAG | 0x7f0));
        combined.push(FilterRule::exact(standard(0x125)).inverted());
        assert!(combined.matches(standard(0x124)));
        assert!(!combined.matches(standard(0x125)));
        assert!(!combined.matches(standard(0x130)));

        assert!(!FilterRule::exact(a).inverted().matches(a));
        assert!(FilterRule::exact(a).inverted().matches(b));
    }
}

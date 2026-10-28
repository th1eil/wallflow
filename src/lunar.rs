//! 农历换算（1900–2100）与常见时区对应的节假日。
//!
//! 农历没有像公历那样的纯算术公式，靠一张 1900–2100 的编码表驱动。每个数值
//! 编码该农历年的闰月月份、12 个普通月与闰月的大小（30/29 天）：
//!
//! - 低 4 位（`& 0xf`）：闰月月份，0 = 无闰月
//! - bit4…bit15（`0x10000 >> m`）：第 m 个月（1..=12）是大月（30）还是小月（29）
//! - bit16（`& 0x10000`）：闰月是大月（30）还是小月（29）
//!
//! 换算起点是「1900 年正月初一 = 公历 1900-01-31」，逐年累加 `lunar_year_days`
//! 定位年份，再逐月递减定位月份（含闰月作为一个独立月）。
//! 主时区/副时区拿到的是自 1970-01-01 的天数（clock.rs 的 `BrokenTime.days`），
//! 因此接口直接吃天数，避免在农历模块里重复做公历拆分。

/// 每个农历年一条编码（1900–2100，共 201 条）。
static LUNAR_INFO: [u32; 201] = [
    0x04bd8, 0x04ae0, 0x0a570, 0x054d5, 0x0d260, 0x0d950, 0x16554, 0x056a0, 0x09ad0, 0x055d2, // 1900
    0x04ae0, 0x0a5b6, 0x0a4d0, 0x0d250, 0x1d255, 0x0b540, 0x0d6a0, 0x0ada2, 0x095b0, 0x14977, // 1910
    0x04970, 0x0a4b0, 0x0b4b5, 0x06a50, 0x06d40, 0x1ab54, 0x02b60, 0x09570, 0x052f2, 0x04970, // 1920
    0x06566, 0x0d4a0, 0x0ea50, 0x06e95, 0x05ad0, 0x02b60, 0x186e3, 0x092e0, 0x1c8d7, 0x0c950, // 1930
    0x0d4a0, 0x1d8a6, 0x0b550, 0x056a0, 0x1a5b4, 0x025d0, 0x092d0, 0x0d2b2, 0x0a950, 0x0b557, // 1940
    0x06ca0, 0x0b550, 0x15355, 0x04da0, 0x0a5d0, 0x14573, 0x052d0, 0x0a9a8, 0x0e950, 0x06aa0, // 1950
    0x0aea6, 0x0ab50, 0x04b60, 0x0aae4, 0x0a570, 0x05260, 0x0f263, 0x0d950, 0x05b57, 0x056a0, // 1960
    0x096d0, 0x04dd5, 0x04ad0, 0x0a4d0, 0x0d4d4, 0x0d250, 0x0d558, 0x0b540, 0x0b5a0, 0x195a6, // 1970
    0x095b0, 0x049b0, 0x0a974, 0x0a4b0, 0x0b27a, 0x06a50, 0x06d40, 0x0af46, 0x0ab60, 0x09570, // 1980
    0x04af5, 0x04970, 0x064b0, 0x074a3, 0x0ea50, 0x06b58, 0x055c0, 0x0ab60, 0x096d5, 0x092e0, // 1990
    0x0c960, 0x0d954, 0x0d4a0, 0x0da50, 0x07552, 0x056a0, 0x0abb7, 0x025d0, 0x092d0, 0x0cab5, // 2000
    0x0a950, 0x0b4a0, 0x0baa4, 0x0ad50, 0x055d9, 0x04ba0, 0x0a5b0, 0x15176, 0x052b0, 0x0a930, // 2010
    0x07954, 0x06aa0, 0x0ad50, 0x05b52, 0x04b60, 0x0a6e6, 0x0a4e0, 0x0d260, 0x0ea65, 0x0d530, // 2020
    0x05aa0, 0x076a3, 0x096d0, 0x04afb, 0x04ad0, 0x0a4d0, 0x1d0b6, 0x0d250, 0x0d520, 0x0dd45, // 2030
    0x0b5a0, 0x056d0, 0x055b2, 0x049b0, 0x0a577, 0x0a4b0, 0x0aa50, 0x1b255, 0x06d20, 0x0ada0, // 2040
    0x14b63, 0x09370, 0x049f8, 0x04970, 0x064b0, 0x168a6, 0x0ea50, 0x06b20, 0x1a6c4, 0x0aae0, // 2050
    0x092e0, 0x0d2e3, 0x0c960, 0x0d557, 0x0d4a0, 0x0da50, 0x05d55, 0x056a0, 0x0a6d0, 0x055d4, // 2060
    0x052d0, 0x0a9b8, 0x0a950, 0x0b4a0, 0x0b6a6, 0x0ad50, 0x055a0, 0x0aba4, 0x0a5b0, 0x052b0, // 2070
    0x0b273, 0x06930, 0x07337, 0x06aa0, 0x0ad50, 0x14b55, 0x04b60, 0x0a570, 0x054e4, 0x0d160, // 2080
    0x0e968, 0x0d520, 0x0daa0, 0x16aa6, 0x056d0, 0x04ae0, 0x0a9d4, 0x0a2d0, 0x0d150, 0x0f252, // 2090
    0x0d520, // 2100
];

const LUNAR_MONTH_NAMES: [&str; 12] = [
    "正月", "二月", "三月", "四月", "五月", "六月",
    "七月", "八月", "九月", "十月", "冬月", "腊月",
];

const LUNAR_DAY_NAMES: [&str; 30] = [
    "初一", "初二", "初三", "初四", "初五", "初六", "初七", "初八", "初九", "初十",
    "十一", "十二", "十三", "十四", "十五", "十六", "十七", "十八", "十九", "二十",
    "廿一", "廿二", "廿三", "廿四", "廿五", "廿六", "廿七", "廿八", "廿九", "三十",
];

/// 一组农历日期。
pub struct LunarDate {
    pub year: i32,
    /// 1..=12（闰月时仍是对应月序号，`is_leap` 标闰）
    pub month: i32,
    /// 1..=30
    pub day: i32,
    pub is_leap: bool,
}

/// Howard Hinnant 的 days_from_civil：公历 → 自 1970-01-01 的天数。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn lunar_info(year: i64) -> u32 {
    LUNAR_INFO[(year - 1900) as usize]
}

fn leap_month(year: i64) -> i32 {
    (lunar_info(year) & 0xf) as i32
}

fn leap_days(year: i64) -> i32 {
    if leap_month(year) != 0 {
        if lunar_info(year) & 0x10000 != 0 { 30 } else { 29 }
    } else {
        0
    }
}

fn month_days(year: i64, month: i32) -> i32 {
    if (1..=12).contains(&month) {
        if lunar_info(year) & (0x10000 >> month) != 0 { 30 } else { 29 }
    } else {
        0
    }
}

fn lunar_year_days(year: i64) -> i64 {
    let info = lunar_info(year);
    let mut sum: i64 = 348; // 12 个月 × 29 天
    let mut i = 0x8000u32;
    while i > 0x8 {
        if info & i != 0 {
            sum += 1;
        }
        i >>= 1;
    }
    sum + leap_days(year) as i64
}

/// 公历天数（自 1970-01-01）→ 农历日期。范围 1900–2100，超出返回 `None`。
pub fn lunar_from_epoch_days(days: i64) -> Option<LunarDate> {
    let base = days_from_civil(1900, 1, 31);
    let mut offset = days - base;
    if offset < 0 {
        return None;
    }

    let mut year: i64 = 1900;
    loop {
        if year > 2100 {
            return None;
        }
        let yd = lunar_year_days(year);
        if offset < yd {
            break;
        }
        offset -= yd;
        year += 1;
    }

    // 逐月定位（闰月作为紧跟其前一个月之后的独立月）
    let leap = leap_month(year);
    let mut month: i32 = 1;
    let mut is_leap = false;
    loop {
        let len = if is_leap {
            leap_days(year)
        } else {
            month_days(year, month)
        } as i64;
        if offset < len {
            return Some(LunarDate {
                year: year as i32,
                month,
                day: (offset + 1) as i32,
                is_leap,
            });
        }
        offset -= len;
        if leap != 0 && month == leap && !is_leap {
            is_leap = true; // 进入闰月
        } else {
            is_leap = false;
            month += 1;
            if month > 12 {
                return None;
            }
        }
    }
}

/// 农历月日的中文写，如「八月十二」「闰八月初一」。
pub fn lunar_month_day(ld: &LunarDate) -> String {
    let m = LUNAR_MONTH_NAMES[(ld.month - 1) as usize];
    let d = LUNAR_DAY_NAMES[(ld.day - 1) as usize];
    if ld.is_leap {
        format!("闰{}{}", m, d)
    } else {
        format!("{}{}", m, d)
    }
}

/// 农历日期对应的传统节日（春节/元宵/端午/中秋/…），无则 `None`。
/// 闰月不计节日；除夕是腊月最后一天（大月三十 / 小月廿九）。
pub fn lunar_festival(ld: &LunarDate) -> Option<&'static str> {
    if ld.is_leap {
        return None;
    }
    match (ld.month, ld.day) {
        (1, 1) => Some("春节"),
        (1, 15) => Some("元宵节"),
        (5, 5) => Some("端午节"),
        (7, 7) => Some("七夕节"),
        (8, 15) => Some("中秋节"),
        (9, 9) => Some("重阳节"),
        (12, 8) => Some("腊八节"),
        (12, d) if d == month_days(ld.year as i64, 12) => Some("除夕"),
        _ => None,
    }
}

/// 各时区固定公历节假日（月, 日, 名称）。浮动节日（复活节、感恩节、第 N 个周 X
/// 等）与宗教历法节日不在内 —— 它们依赖每年的日历数据，超出本表范围。
static FIXED_HOLIDAYS: &[(&str, &[(u32, u32, &str)])] = &[
    ("Asia/Shanghai", &[
        (1, 1, "元旦"), (3, 8, "妇女节"), (5, 1, "劳动节"), (5, 4, "青年节"),
        (6, 1, "儿童节"), (7, 1, "建党节"), (8, 1, "建军节"), (9, 10, "教师节"),
        (10, 1, "国庆节"),
    ]),
    ("Asia/Tokyo", &[
        (1, 1, "元日"), (2, 11, "建国纪念日"), (2, 23, "天皇诞辰"), (3, 3, "女儿节"),
        (4, 29, "昭和日"), (5, 3, "宪法纪念日"), (5, 4, "绿之日"), (5, 5, "儿童节"),
        (8, 11, "山之日"), (11, 3, "文化日"), (11, 23, "勤劳感谢日"),
    ]),
    ("Asia/Seoul", &[
        (1, 1, "元旦"), (3, 1, "三一节"), (5, 5, "儿童节"), (6, 6, "显忠日"),
        (8, 15, "光复节"), (10, 3, "开天节"), (10, 9, "韩文日"), (12, 25, "圣诞节"),
    ]),
    ("Asia/Singapore", &[
        (1, 1, "元旦"), (8, 9, "国庆日"), (12, 25, "圣诞节"),
    ]),
    ("Asia/Dubai", &[
        (1, 1, "元旦"), (12, 2, "国庆日"),
    ]),
    ("Europe/London", &[
        (1, 1, "元旦"), (2, 14, "情人节"), (3, 17, "圣帕特里克节"), (4, 23, "圣乔治节"),
        (12, 25, "圣诞节"), (12, 26, "节礼日"),
    ]),
    ("Europe/Paris", &[
        (1, 1, "元旦"), (2, 14, "情人节"), (5, 1, "劳动节"), (5, 8, "胜利纪念日"),
        (7, 14, "国庆日"), (8, 15, "圣母升天节"), (11, 1, "诸圣节"), (11, 11, "停战纪念日"),
        (12, 25, "圣诞节"),
    ]),
    ("Europe/Moscow", &[
        (1, 1, "元旦"), (1, 7, "东正教圣诞节"), (2, 23, "祖国保卫者日"), (3, 8, "妇女节"),
        (5, 1, "劳动节"), (5, 9, "胜利日"), (6, 12, "俄罗斯日"), (11, 4, "民族团结日"),
    ]),
    ("Australia/Sydney", &[
        (1, 1, "元旦"), (1, 26, "澳大利亚日"), (4, 25, "澳新军团日"),
        (12, 25, "圣诞节"), (12, 26, "节礼日"),
    ]),
    ("America/New_York", &[
        (1, 1, "元旦"), (2, 14, "情人节"), (7, 4, "独立日"), (10, 31, "万圣节"),
        (11, 11, "退伍军人节"), (12, 25, "圣诞节"),
    ]),
    ("America/Los_Angeles", &[
        (1, 1, "元旦"), (2, 14, "情人节"), (7, 4, "独立日"), (10, 31, "万圣节"),
        (11, 11, "退伍军人节"), (12, 25, "圣诞节"),
    ]),
];

/// 该时区在某天的节假日名。中国（大陆/港/台）优先农历节日，再回退固定公历节日。
/// 未收录的时区返回 `None`。
pub fn holiday_for_tz(tz: &str, month: u32, day: u32, epoch_days: i64) -> Option<&'static str> {
    if matches!(tz, "Asia/Shanghai" | "Asia/Hong_Kong" | "Asia/Taipei" | "Asia/Macau") {
        if let Some(ld) = lunar_from_epoch_days(epoch_days) {
            if let Some(name) = lunar_festival(&ld) {
                return Some(name);
            }
        }
    }
    let list = FIXED_HOLIDAYS.iter().find(|(t, _)| *t == tz)?.1;
    list.iter()
        .find(|(m, d, _)| *m == month && *d == day)
        .map(|(_, _, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epoch_days(y: i64, m: i64, d: i64) -> i64 {
        days_from_civil(y, m, d)
    }

    /// 断言某公历日期是农历 (year, month, day, is_leap)。
    fn assert_lunar(greg: (i64, i64, i64), lunar: (i32, i32, i32, bool)) {
        let ld = lunar_from_epoch_days(epoch_days(greg.0, greg.1, greg.2)).unwrap();
        assert_eq!(
            (ld.year, ld.month, ld.day, ld.is_leap),
            lunar,
            "公历 {:?} 的农历不符",
            greg
        );
    }

    #[test]
    fn test_spring_festivals() {
        // 正月初一（春节）在公历上的已知日期。
        assert_lunar((2021, 2, 12), (2021, 1, 1, false));
        assert_lunar((2022, 2, 1), (2022, 1, 1, false));
        assert_lunar((2023, 1, 22), (2023, 1, 1, false));
        assert_lunar((2024, 2, 10), (2024, 1, 1, false));
        assert_lunar((2025, 1, 29), (2025, 1, 1, false));
        assert_lunar((2026, 2, 17), (2026, 1, 1, false));
    }

    #[test]
    fn test_month_day_format() {
        let ld = lunar_from_epoch_days(epoch_days(2025, 1, 29)).unwrap();
        assert_eq!(lunar_month_day(&ld), "正月初一");
        assert_eq!(lunar_festival(&ld), Some("春节"));
    }

    #[test]
    fn test_holiday() {
        // 中国国庆
        assert_eq!(holiday_for_tz("Asia/Shanghai", 10, 1, 0), Some("国庆节"));
        // 美国独立日
        assert_eq!(holiday_for_tz("America/New_York", 7, 4, 0), Some("独立日"));
        // 非节日
        assert_eq!(holiday_for_tz("America/New_York", 7, 5, 0), None);
        // 未收录时区
        assert_eq!(holiday_for_tz("Europe/Oslo", 12, 25, 0), None);
    }
}
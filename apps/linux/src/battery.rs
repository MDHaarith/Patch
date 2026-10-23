//! System battery readings from Linux power supplies, when available.

fn reading(uevent: &str) -> Option<String> {
    let field = |key: &str| {
        uevent
            .lines()
            .filter_map(|line| line.split_once('='))
            .find_map(|(name, value)| (name == key).then_some(value))
    };
    if field("POWER_SUPPLY_TYPE") != Some("Battery")
        || field("POWER_SUPPLY_SCOPE") == Some("Device")
        || field("POWER_SUPPLY_PRESENT") == Some("0")
    {
        return None;
    }
    let level: u8 = field("POWER_SUPPLY_CAPACITY")?.parse().ok()?;
    if level > 100 {
        return None;
    }
    let name = field("POWER_SUPPLY_NAME").unwrap_or("system");
    let mut text = format!("Device battery {name}: {level}%");
    if let Some(status) = field("POWER_SUPPLY_STATUS") {
        text.push_str(&format!(" · {}", status.to_lowercase()));
    }
    if let Some(current) = field("POWER_SUPPLY_CURRENT_NOW")
        .and_then(|v| v.parse::<i64>().ok())
        .and_then(i64::checked_abs)
    {
        text.push_str(&format!(" · {} mA", current / 1000));
    }
    Some(text)
}

pub fn sample() -> Option<String> {
    let mut readings: Vec<String> = std::fs::read_dir("/sys/class/power_supply")
        .ok()?
        .flatten()
        .filter_map(|entry| reading(&std::fs::read_to_string(entry.path().join("uevent")).ok()?))
        .collect();
    readings.sort_unstable();
    (!readings.is_empty()).then(|| readings.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_system_batteries_and_ignores_unavailable_values() {
        let battery = "POWER_SUPPLY_TYPE=Battery\nPOWER_SUPPLY_NAME=BAT0\nPOWER_SUPPLY_CAPACITY=76\nPOWER_SUPPLY_STATUS=Discharging\nPOWER_SUPPLY_CURRENT_NOW=-120000\n";
        assert_eq!(
            reading(battery).as_deref(),
            Some("Device battery BAT0: 76% · discharging · 120 mA")
        );
        assert_eq!(
            reading("POWER_SUPPLY_TYPE=Battery\nPOWER_SUPPLY_CAPACITY=0").as_deref(),
            Some("Device battery system: 0%")
        );
        for extra in [
            "POWER_SUPPLY_SCOPE=Device",
            "POWER_SUPPLY_PRESENT=0",
            "POWER_SUPPLY_CAPACITY=-1",
            "POWER_SUPPLY_CAPACITY=101",
            "POWER_SUPPLY_CAPACITY=unknown",
        ] {
            assert_eq!(reading(&format!("{extra}\n{battery}")), None);
        }
        assert_eq!(reading("POWER_SUPPLY_TYPE=Mains"), None);
        assert_eq!(reading("POWER_SUPPLY_TYPE=Battery"), None);
        assert_eq!(
            reading(&battery.replace("Discharging", "Charging").replace("-120000", "120000")).as_deref(),
            Some("Device battery BAT0: 76% · charging · 120 mA")
        );
        assert_eq!(
            reading(&battery.replace("-120000", "unknown")).as_deref(),
            Some("Device battery BAT0: 76% · discharging")
        );
        assert_eq!(
            reading(&battery.replace("-120000", "-9223372036854775808")).as_deref(),
            Some("Device battery BAT0: 76% · discharging")
        );
    }
}

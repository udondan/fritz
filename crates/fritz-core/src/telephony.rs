//! The telephony of a device: its numbers and internet telephony accounts,
//! call deflections, answering machines and DECT handsets. Not the calls
//! and the phonebook, which are personal.

use serde::Serialize;

use crate::generic::Error;
use crate::lists::{self, Item};
use crate::read::Reader;

/// The most DECT handsets read, as a guard against a device that never
/// ends the list.
pub const MAX_HANDSETS: u32 = 100;

/// The telephony.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Telephony {
    pub numbers: Vec<Number>,
    pub accounts: Vec<Account>,
    pub deflections: Vec<Deflection>,
    pub answering_machines: Vec<AnsweringMachine>,
    pub handsets: Vec<Handset>,
}

/// A number of the device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Number {
    pub number: Option<String>,
    /// Like `eVoIP` or `eAnalog`.
    pub kind: Option<String>,
    pub name: Option<String>,
}

/// An internet telephony account with a number. Without its user name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Account {
    pub index: u32,
    pub number: String,
    pub registrar: Option<String>,
    /// Like `registered` or `not registered`.
    pub status: Option<String>,
}

/// A call deflection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Deflection {
    pub enabled: bool,
    /// What the deflection is for, like `fromNumber` or `toAny`.
    pub kind: Option<String>,
    /// The number it is for, if any.
    pub number: Option<String>,
    /// The number calls go to.
    pub to: Option<String>,
    /// Like `eImmediately` or `eDelayed`.
    pub mode: Option<String>,
}

/// An answering machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AnsweringMachine {
    pub index: u32,
    pub name: Option<String>,
    pub enabled: bool,
}

/// A DECT handset.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Handset {
    pub id: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    /// Whether it is in reach.
    pub active: bool,
    pub update_available: Option<bool>,
}

/// Reads the telephony.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`], and [`Error::InvalidDocument`] if a list is not
/// valid.
pub async fn telephony(reader: &mut Reader) -> Result<Telephony, Error> {
    let numbers = list(reader, ("X_VoIP1", "X_AVM-DE_GetNumbers"), "NewNumberList")
        .await?
        .iter()
        .map(|item| Number {
            number: item.text("Number"),
            kind: item.text("Type"),
            name: item.text("Name"),
        })
        .collect();
    Ok(Telephony {
        numbers,
        accounts: accounts(reader).await?,
        deflections: list(
            reader,
            ("X_AVM-DE_OnTel1", "GetDeflections"),
            "NewDeflectionList",
        )
        .await?
        .iter()
        .map(|item| Deflection {
            enabled: item.flag("Enable").unwrap_or_default(),
            kind: item.text("Type"),
            number: item.text("Number"),
            to: item.text("DeflectionToNumber"),
            mode: item.text("Mode"),
        })
        .collect(),
        answering_machines: list(reader, ("X_AVM-DE_TAM1", "GetList"), "NewTAMList")
            .await?
            .iter()
            .filter(|item| item.flag("Display").unwrap_or_default())
            .map(|item| AnsweringMachine {
                index: item.number("Index").unwrap_or_default(),
                name: item.text("Name"),
                enabled: item.flag("Enable").unwrap_or_default(),
            })
            .collect(),
        handsets: handsets(reader).await?,
    })
}

/// The items of a `<List>` that `action` returns in the output `output`.
async fn list(
    reader: &mut Reader,
    (service, action): (&str, &'static str),
    output: &str,
) -> Result<Vec<Item>, Error> {
    let Some(xml) = reader
        .try_read(service, action, &[])
        .await?
        .and_then(|values| values.text(output))
    else {
        return Ok(Vec::new());
    };
    lists::items(&xml, action, "List", "Item")
}

/// The accounts with a number, each with its status.
async fn accounts(reader: &mut Reader) -> Result<Vec<Account>, Error> {
    let Some(xml) = reader
        .try_read("X_VoIP1", "X_AVM-DE_GetVoIPAccounts", &[])
        .await?
        .and_then(|values| values.text("NewX_AVM-DE_VoIPAccountList"))
    else {
        return Ok(Vec::new());
    };
    let mut accounts = Vec::new();
    for item in lists::items(&xml, "VoIP account list", "AccountList", "Account")? {
        let Some(number) = item.text("VoIPNumber") else {
            continue;
        };
        let index: u32 = item.number("VoIPAccountIndex").unwrap_or_default();
        let status = reader
            .try_read(
                "X_VoIP1",
                "X_AVM-DE_GetVoIPStatus",
                &[("NewVoIPAccountIndex", &index.to_string())],
            )
            .await?
            .and_then(|values| values.text("NewX_AVM-DE_VoIPStatus"));
        accounts.push(Account {
            index,
            number,
            registrar: item.text("VoIPRegistrar"),
            status,
        });
    }
    Ok(accounts)
}

async fn handsets(reader: &mut Reader) -> Result<Vec<Handset>, Error> {
    let entries = reader
        .entries(
            "X_AVM-DE_Dect1",
            ("GetNumberOfDectEntries", "NewNumberOfEntries"),
            ("GetGenericDectEntry", "NewIndex"),
            MAX_HANDSETS,
        )
        .await?
        .map(|(_, entries)| entries)
        .unwrap_or_default();
    Ok(entries
        .iter()
        .map(|entry| Handset {
            id: entry.text("NewID"),
            name: entry.text("NewName"),
            model: entry.text("NewModel"),
            active: entry.flag("NewActive").unwrap_or_default(),
            update_available: entry.flag("NewUpdateAvailable"),
        })
        .collect())
}

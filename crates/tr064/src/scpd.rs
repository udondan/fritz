//! Service control protocol descriptions (SCPD): the actions of a service,
//! their arguments, and the state variables that give the arguments their
//! types.

use roxmltree::{Document, Node};

use crate::{Error, xml};

/// A parsed SCPD document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scpd {
    pub actions: Vec<Action>,
    pub state_variables: Vec<StateVariable>,
}

/// An action of a service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub name: String,
    pub arguments: Vec<Argument>,
}

/// An argument of an [`Action`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argument {
    pub name: String,
    pub direction: Direction,
    /// The name of the [`StateVariable`] that gives the argument its type.
    pub related_state_variable: String,
}

/// Whether an [`Argument`] is sent to the device or returned by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
}

/// A state variable of a service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateVariable {
    pub name: String,
    /// The UPnP data type, for example `string`, `ui4` or `boolean`.
    pub data_type: String,
    pub default_value: Option<String>,
    /// The allowed values; empty if any value of the data type is allowed.
    pub allowed_values: Vec<String>,
    pub allowed_range: Option<AllowedRange>,
    pub send_events: bool,
}

/// The allowed range of a numeric [`StateVariable`], as given in the SCPD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedRange {
    pub minimum: Option<String>,
    pub maximum: Option<String>,
    pub step: Option<String>,
}

impl Scpd {
    /// Parses an SCPD document.
    pub fn parse(xml: &str) -> Result<Self, Error> {
        let doc = Document::parse(xml)?;
        let root = xml::root(&doc, "scpd")?;
        Ok(Self {
            actions: xml::list(root, "actionList", "action", parse_action)?,
            state_variables: xml::list(
                root,
                "serviceStateTable",
                "stateVariable",
                parse_state_variable,
            )?,
        })
    }

    /// The action with the given name.
    pub fn action(&self, name: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.name == name)
    }

    /// The state variable with the given name.
    pub fn state_variable(&self, name: &str) -> Option<&StateVariable> {
        self.state_variables.iter().find(|v| v.name == name)
    }
}

impl Action {
    /// The arguments sent to the device.
    pub fn inputs(&self) -> impl Iterator<Item = &Argument> {
        self.arguments
            .iter()
            .filter(|a| a.direction == Direction::In)
    }

    /// The arguments returned by the device.
    pub fn outputs(&self) -> impl Iterator<Item = &Argument> {
        self.arguments
            .iter()
            .filter(|a| a.direction == Direction::Out)
    }
}

fn parse_action(node: Node<'_, '_>) -> Result<Action, Error> {
    Ok(Action {
        name: xml::required(node, "action", "name")?,
        arguments: xml::list(node, "argumentList", "argument", parse_argument)?,
    })
}

fn parse_argument(node: Node<'_, '_>) -> Result<Argument, Error> {
    let direction = xml::required(node, "argument", "direction")?;
    Ok(Argument {
        name: xml::required(node, "argument", "name")?,
        direction: match direction.to_ascii_lowercase().as_str() {
            "in" => Direction::In,
            "out" => Direction::Out,
            _ => {
                return Err(Error::InvalidValue {
                    element: "direction",
                    value: direction,
                });
            }
        },
        related_state_variable: xml::required(node, "argument", "relatedStateVariable")?,
    })
}

fn parse_state_variable(node: Node<'_, '_>) -> Result<StateVariable, Error> {
    let allowed_values = xml::child(node, "allowedValueList").map_or_else(Vec::new, |list| {
        list.children()
            .filter(|c| c.is_element() && c.tag_name().name() == "allowedValue")
            .filter_map(|c| c.text())
            .map(|t| t.trim().to_owned())
            .collect()
    });
    let allowed_range = xml::child(node, "allowedValueRange").map(|range| AllowedRange {
        minimum: xml::optional(range, "minimum"),
        maximum: xml::optional(range, "maximum"),
        step: xml::optional(range, "step"),
    });
    Ok(StateVariable {
        name: xml::required(node, "stateVariable", "name")?,
        data_type: xml::required(node, "stateVariable", "dataType")?,
        default_value: xml::optional(node, "defaultValue"),
        allowed_values,
        allowed_range,
        // UPnP: events are sent unless the attribute says "no".
        send_events: node.attribute("sendEvents") != Some("no"),
    })
}

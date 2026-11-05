//! The page tree (ISO 32000-2 §7.7.3): every page in order, with the attributes it inherits.

use std::collections::BTreeSet;
use std::rc::Rc;

use super::file::File;
use super::object::{Object, Ref};
use super::{Error, Limit};

/// A rectangle in default user space, normalized so that `x0 < x1` and `y0 < y1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x0: f64,
    pub(crate) y0: f64,
    pub(crate) x1: f64,
    pub(crate) y1: f64,
}

/// A page and the attributes it uses, inherited ones included.
#[derive(Clone, Debug)]
pub(crate) struct PageInfo {
    /// The page's number, counted from 1.
    pub(crate) number: usize,
    /// The page dictionary.
    pub(crate) dict: Rc<Object>,
    /// The page's resource dictionary (or a reference to it), if it has one: shared with the node that holds it.
    pub(crate) resources: Option<Attribute>,
    /// The visible area: the crop box, clipped to the media box.
    pub(crate) visible: Rect,
    /// The clockwise rotation for display: 0, 90, 180 or 270 degrees.
    pub(crate) rotate: u16,
}

/// An attribute of a page, held by the page or by the page tree node it inherits it from, and shared with that node rather than copied: however many pages inherit a large attribute, it is in memory once.
#[derive(Clone, Debug)]
pub(crate) struct Attribute {
    /// The node that holds it, as the file's cache keeps it.
    node: Rc<Object>,
    /// Its key in the node's dictionary.
    key: &'static [u8],
}

impl Attribute {
    /// The attribute of `node` under `key`, if the node has one.
    fn of(node: &Rc<Object>, key: &'static [u8]) -> Option<Self> {
        node.as_dict()
            .is_some_and(|dict| dict.contains(key))
            .then(|| Self {
                node: Rc::clone(node),
                key,
            })
    }

    /// The attribute's value as the node holds it: an object or a reference to one.
    pub(crate) fn value(&self) -> Option<&Object> {
        self.node.as_dict().and_then(|dict| dict.get(self.key))
    }
}

/// The attributes a page inherits from its ancestors (§7.7.3.4).
#[derive(Clone, Default)]
struct Inherited {
    resources: Option<Attribute>,
    media_box: Option<Attribute>,
    crop_box: Option<Attribute>,
    rotate: Option<Attribute>,
}

impl Inherited {
    /// The attributes below `node`: its own, else those it inherits.
    fn update(&self, node: &Rc<Object>) -> Self {
        let pick = |key: &'static [u8], current: &Option<Attribute>| {
            Attribute::of(node, key).or_else(|| current.clone())
        };
        Self {
            resources: pick(b"Resources", &self.resources),
            media_box: pick(b"MediaBox", &self.media_box),
            crop_box: pick(b"CropBox", &self.crop_box),
            rotate: pick(b"Rotate", &self.rotate),
        }
    }
}

/// Every page of the document, in order.
///
/// # Errors
///
/// [`Error::Malformed`] for a broken catalog or page tree, [`Error::Unsupported`] for optional content and user units, [`Error::Limit`] past [`super::Limits::max_pages`] or [`super::Limits::max_nesting`].
pub(crate) fn collect(file: &mut File<'_>) -> Result<Vec<PageInfo>, Error> {
    let root = match file.trailer().get(b"Root") {
        Some(Object::Ref(root)) => *root,
        _ => {
            return Err(Error::malformed(
                "the trailer has no reference to the catalog (`Root`)",
            ));
        }
    };
    let catalog = file.get(root)?;
    let catalog = catalog
        .as_dict()
        .filter(|dict| dict.has_name(b"Type", b"Catalog"))
        .ok_or_else(|| {
            Error::malformed("the document catalog is not a dictionary of type `Catalog`")
        })?;
    if catalog.contains(b"OCProperties") {
        return Err(Error::unsupported(
            "optional content (layers), which could hide text that the extractor would report",
        ));
    }
    let tree = match catalog.get(b"Pages") {
        Some(Object::Ref(tree)) => *tree,
        _ => {
            return Err(Error::malformed(
                "the catalog has no reference to the page tree (`Pages`)",
            ));
        }
    };
    let mut pages = Vec::new();
    let mut visited = BTreeSet::new();
    walk(
        file,
        tree,
        &Inherited::default(),
        0,
        &mut visited,
        &mut pages,
    )?;
    Ok(pages)
}

/// Adds the pages under the node `node`.
fn walk(
    file: &mut File<'_>,
    node: Ref,
    inherited: &Inherited,
    depth: usize,
    visited: &mut BTreeSet<Ref>,
    pages: &mut Vec<PageInfo>,
) -> Result<(), Error> {
    if depth >= file.limits().max_nesting {
        return Err(Error::Limit(Limit::Nesting));
    }
    if !visited.insert(node) {
        return Err(Error::malformed(format!(
            "object {} appears twice in the page tree",
            node.number
        )));
    }
    let object = file.get(node)?;
    let dict = object
        .as_dict()
        .filter(|_| matches!(&*object, Object::Dict(_)))
        .ok_or_else(|| {
            Error::malformed(format!(
                "page tree node {} is not a dictionary",
                node.number
            ))
        })?;
    let inherited = inherited.update(&object);
    if dict.has_name(b"Type", b"Pages") {
        let kids = match dict.get(b"Kids") {
            Some(kids) => file.resolve(kids)?,
            None => {
                return Err(Error::malformed(format!(
                    "page tree node {} has no `Kids`",
                    node.number
                )));
            }
        };
        let kids = kids.as_array().ok_or_else(|| {
            Error::malformed(format!(
                "the `Kids` of page tree node {} are not an array",
                node.number
            ))
        })?;
        for kid in kids {
            let &Object::Ref(kid) = kid else {
                return Err(Error::malformed(format!(
                    "page tree node {} has a kid that is not a reference",
                    node.number
                )));
            };
            walk(file, kid, &inherited, depth + 1, visited, pages)?;
        }
        Ok(())
    } else if dict.has_name(b"Type", b"Page") {
        if pages.len() >= file.limits().max_pages {
            return Err(Error::Limit(Limit::Pages));
        }
        let number = pages.len() + 1;
        let page =
            page_info(file, &object, &inherited, number).map_err(|error| error.on_page(number))?;
        pages.push(page);
        Ok(())
    } else {
        Err(Error::malformed(format!(
            "page tree node {} is neither of type `Pages` nor of type `Page`",
            node.number
        )))
    }
}

/// The geometry and resources of one page.
fn page_info(
    file: &mut File<'_>,
    object: &Rc<Object>,
    inherited: &Inherited,
    number: usize,
) -> Result<PageInfo, Error> {
    let dict = object
        .as_dict()
        .ok_or_else(|| Error::malformed("the page is not a dictionary"))?;
    if let Some(unit) = file.lookup(dict, b"UserUnit")?
        && unit.as_number() != Some(1.0)
    {
        return Err(Error::unsupported("a user unit other than 1 (`UserUnit`)"));
    }
    let media_box = match inherited.media_box.as_ref().and_then(Attribute::value) {
        Some(object) => rect(file, object)?,
        None => return Err(Error::malformed("the page has no media box")),
    };
    let crop_box = match inherited.crop_box.as_ref().and_then(Attribute::value) {
        Some(object) => rect(file, object)?,
        None => media_box,
    };
    // The crop box is clipped to the media box (§14.11.2).
    let visible = Rect {
        x0: if crop_box.x0 > media_box.x0 {
            crop_box.x0
        } else {
            media_box.x0
        },
        y0: if crop_box.y0 > media_box.y0 {
            crop_box.y0
        } else {
            media_box.y0
        },
        x1: if crop_box.x1 < media_box.x1 {
            crop_box.x1
        } else {
            media_box.x1
        },
        y1: if crop_box.y1 < media_box.y1 {
            crop_box.y1
        } else {
            media_box.y1
        },
    };
    if visible.x1 <= visible.x0 || visible.y1 <= visible.y0 {
        return Err(Error::malformed(
            "the page's crop box lies outside its media box",
        ));
    }
    let rotate = match inherited.rotate.as_ref().and_then(Attribute::value) {
        None => 0,
        Some(object) => {
            let value = file
                .resolve(object)?
                .as_integer()
                .ok_or_else(|| Error::malformed("the page's `Rotate` is not an integer"))?;
            if value % 90 != 0 {
                return Err(Error::malformed(
                    "the page's `Rotate` is not a multiple of 90",
                ));
            }
            u16::try_from(value.rem_euclid(360))
                .map_err(|_| Error::malformed("the page's `Rotate` is out of range"))?
        }
    };
    Ok(PageInfo {
        number,
        dict: Rc::clone(object),
        resources: inherited.resources.clone(),
        visible,
        rotate,
    })
}

/// A rectangle: an array of four numbers, in any order of corners (§7.9.5).
fn rect(file: &mut File<'_>, object: &Object) -> Result<Rect, Error> {
    let value = file.resolve(object)?;
    let items = value
        .as_array()
        .filter(|items| items.len() == 4)
        .ok_or_else(|| Error::malformed("a page box is not an array of four numbers"))?;
    let mut numbers = [0.0; 4];
    for (slot, item) in numbers.iter_mut().zip(items) {
        *slot = file
            .resolve(item)?
            .as_number()
            .filter(|number| number.is_finite())
            .ok_or_else(|| Error::malformed("a page box is not an array of four numbers"))?;
    }
    let [ax, ay, bx, by] = numbers;
    Ok(Rect {
        x0: if ax < bx { ax } else { bx },
        y0: if ay < by { ay } else { by },
        x1: if ax < bx { bx } else { ax },
        y1: if ay < by { by } else { ay },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::object::Dict;
    use crate::pdf::{Limits, test_pdf};

    #[test]
    fn inherited_attributes_are_shared_not_copied() {
        // The root node holds the resources and the media box directly; two of its three pages inherit them.
        let data = test_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 600 800] /Resources << /Font << /F1 6 0 R >> >> >>",
            "<< /Type /Page /Parent 2 0 R >>",
            "<< /Type /Page /Parent 2 0 R >>",
            "<< /Type /Page /Parent 2 0 R /Resources << >> >>",
            "<< /Type /Font >>",
        ]);
        let mut file = File::open(&data, Limits::DEFAULT).unwrap();
        let pages = collect(&mut file).unwrap();
        let resources: Vec<&Object> = pages
            .iter()
            .map(|page| page.resources.as_ref().and_then(Attribute::value).unwrap())
            .collect();
        // The pages that inherit the resources see the very object of the root node, not copies of it.
        assert!(core::ptr::eq(resources[0], resources[1]));
        let root = file
            .get(Ref {
                number: 2,
                generation: 0,
            })
            .unwrap();
        assert!(core::ptr::eq(
            resources[0],
            root.as_dict().unwrap().get(b"Resources").unwrap()
        ));
        // A page's own attribute replaces the inherited one.
        assert!(!core::ptr::eq(resources[0], resources[2]));
        assert_eq!(resources[2], &Object::Dict(Dict::default()));
        let whole = Rect {
            x0: 0.0,
            y0: 0.0,
            x1: 600.0,
            y1: 800.0,
        };
        assert!(pages.iter().all(|page| page.visible == whole));
    }
}

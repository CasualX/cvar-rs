/*!
Interact with the configuration variables.

The design of this library makes recursive depth-first pre-order traversal the only feasable method to walk the cvars.

This trade-off allows the hierarchy to be constructed lazily with very convenient stack-allocated resources.
!*/

use super::*;

use std::str;

//----------------------------------------------------------------

/// Tokenise command-like argument strings.
///
/// ```
/// use cvar::console::tokenise;
/// let tokens = tokenise("command 42 'string arg with spaces'").collect::<Vec<_>>();
/// assert_eq!(tokens, &["command", "42", "string arg with spaces"]);
/// ```
pub fn tokenise<'a>(s: &'a str) -> impl Clone + Iterator<Item = &'a str> {
	#[derive(Clone)]
	struct Tokeniser<'a>(str::Chars<'a>);
	impl<'a> Iterator for Tokeniser<'a> {
		type Item = &'a str;
		fn next(&mut self) -> Option<&'a str> {
			// Keep track of the start of the token
			let mut s = self.0.as_str();
			loop {
				match self.0.next() {
					// Skip all whitespace
					Some(chr) => {
						if !(chr == ' ' || chr == '\t') {
							break;
						}
						s = self.0.as_str();
					},
					// Only whitespace or EOS
					None => return None,
				}
			}
			match self.0.next() {
				// Handle quoted arguments until end quote
				Some(quote @ '\'') | Some(quote @ '\"') => {
					while let Some(chr) = self.0.next() {
						if chr == quote {
							break;
						}
					}
				},
				// Unquoted arguments until whitespace or EOS
				_ => {
					while let Some(chr) = self.0.next() {
						if chr == ' ' || chr == '\t' {
							break;
						}
					}
				},
			}
			let len = self.0.as_str().as_ptr() as usize - s.as_ptr() as usize;
			Some(&s[..len])
		}
	}
	Tokeniser(s.chars())
}

//----------------------------------------------------------------

/// Sets a property its value.
#[inline]
pub fn set(root: &mut IVisit, path: &str, val: &str) -> BoxResult<bool> {
	let mut result = Ok(false);
	find(root, path, |node| {
		if let NodeMut::Prop(prop) = node.as_node_mut() {
			result = prop.set(val).map(|_| true);
		}
	});
	result
}

/// Gets a property its value.
#[inline]
pub fn get(root: &mut IVisit, path: &str) -> Option<String> {
	let mut result = None;
	find(root, path, |node| {
		if let NodeMut::Prop(prop) = node.as_node_mut() {
			result = Some(prop.get());
		}
	});
	result
}

/// Resets properties to their default.
///
/// Given a list node will reset all its children to their default. Ignores action nodes.
pub fn reset(root: &mut IVisit, path: &str) -> bool {
	find(root, path, |node| {
		match node.as_node_mut() {
			NodeMut::Prop(prop) => prop.reset(),
			NodeMut::List(list) => reset_all(list.as_visit_mut()),
			NodeMut::Action(_) => (),
		}
	})
}
/// Resets all properties to their default.
pub fn reset_all(root: &mut IVisit) {
	root.visit_mut(&mut |node| {
		match node.as_node_mut() {
			NodeMut::Prop(prop) => prop.reset(),
			NodeMut::List(list) => reset_all(list.as_visit_mut()),
			NodeMut::Action(_) => (),
		}
	});
}

//----------------------------------------------------------------

/// Compares a path to a node name.
///
/// * `ComparePath::True` if they compare equal.
/// * `ComparePath::Part` if the path starts with name and is followed by `.`.
/// * `ComparePath::False` in all other cases.
///
/// Node names are allowed to contain `.`, eg: `Action::new("foo.bar", "desc", |ctx| Ok(()))`.
/// This does not create a `foo` list node, but merely allows the node to pretend to be part of one.
///
/// Node names are allowed to be the empty string, while confusing there's nothing special about it.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ComparePath<'a> {
	False,
	True,
	Part(&'a str),
}
impl<'a> ComparePath<'a> {
	fn cmp(path: &'a str, name: &str) -> ComparePath<'a> {
		match path.as_bytes().get(name.len()) {
			Some(&b'.') => {
				if &path.as_bytes()[..name.len()] == name.as_bytes() {
					ComparePath::Part(&path[name.len() + 1..])
				}
				else {
					ComparePath::False
				}
			},
			Some(_) => ComparePath::False,
			None => if path == name { ComparePath::True } else { ComparePath::False },
		}
	}
}
#[test]
fn test_compare_id() {
	assert_eq!(ComparePath::cmp("foo", "foo"), ComparePath::True);
	assert_eq!(ComparePath::cmp("foo.bar", "foo"), ComparePath::Part("bar"));
	assert_eq!(ComparePath::cmp("foo.bar", "foo.bar"), ComparePath::True);
	assert_eq!(ComparePath::cmp("foo.bar.baz", "foo.bar"), ComparePath::Part("baz"));
	assert_eq!(ComparePath::cmp("fooz", "foo"), ComparePath::False);
	assert_eq!(ComparePath::cmp("foo.bar.baz", "bar.baz"), ComparePath::False);
	assert_eq!(ComparePath::cmp("foo", "foo.bar"), ComparePath::False);
	// Degenerate cases: empty string node names
	assert_eq!(ComparePath::cmp("foo.", "foo"), ComparePath::Part(""));
	assert_eq!(ComparePath::cmp("foo.", "foo."), ComparePath::True);
	assert_eq!(ComparePath::cmp("foo", "foo."), ComparePath::False);
	assert_eq!(ComparePath::cmp("", ""), ComparePath::True);
	assert_eq!(ComparePath::cmp(".", ""), ComparePath::Part(""));
	assert_eq!(ComparePath::cmp(".", "."), ComparePath::True);
	assert_eq!(ComparePath::cmp("", "."), ComparePath::False);
	assert_eq!(ComparePath::cmp(".foo", "foo"), ComparePath::False);
	assert_eq!(ComparePath::cmp(".foo", ".foo"), ComparePath::True);
	assert_eq!(ComparePath::cmp("foo", ".foo"), ComparePath::False);
}

//----------------------------------------------------------------

/// Finds a cvar by its path and invokes the closure with it.
///
/// If there are multiple nodes with the same name, the closure is called for every match.
/// This allows interesting features where eg. the same path is both an action and a property.
///
/// Returns false if no nodes were found with this path, the closure has not been called.
#[inline]
pub fn find<F: FnMut(&mut INode)>(root: &mut IVisit, path: &str, mut f: F) -> bool {
	find_rec(root, path, &mut f)
}
fn find_rec(list: &mut IVisit, path: &str, f: &mut FnMut(&mut INode)) -> bool {
	let mut found = false;
	list.visit_mut(&mut |node| {
		match ComparePath::cmp(path, node.name()) {
			ComparePath::True => {
				f(node);
				found = true;
			},
			ComparePath::Part(tail) => {
				if let NodeMut::List(list) = node.as_node_mut() {
					found |= find_rec(list.as_visit_mut(), tail, f);
				}
			},
			ComparePath::False => {},
		};
	});
	found
}

/// Walks all nodes in the cvar hierarchy and call the closure with the node along its full path.
#[inline]
pub fn walk<F: FnMut(&str, &mut INode)>(root: &mut IVisit, mut f: F) {
	let mut path = String::new();
	walk_rec(root, &mut path, &mut f);
}
fn walk_rec(list: &mut IVisit, path: &mut String, f: &mut FnMut(&str, &mut INode)) {
	list.visit_mut(&mut |node| {
		// Construct the path
		let len = path.len();
		if len > 0 {
			path.push('.');
		}
		path.push_str(node.name());
		// Tell our caller about the node
		f(&path, node);
		// Recursively visit list nodes
		if let NodeMut::List(list) = node.as_node_mut() {
			walk_rec(list.as_visit_mut(), path, f);
		}
		// Pop off the name so we can reuse this string
		path.truncate(len);
	});
}

//----------------------------------------------------------------

/// Invokes an action.
#[inline]
pub fn invoke(root: &mut IVisit, path: &str, args: &[&str], console: &mut IConsole) -> bool {
	let mut found = false;
	find(root, path, |node| {
		if let NodeMut::Action(act) = node.as_node_mut() {
			found = true;
			act.invoke(args, console);
		}
	});
	found
}

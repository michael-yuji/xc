// Copyright (c) 2023 Yan Ka, Chiu.
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
// 1. Redistributions of source code must retain the above copyright
//    notice, this list of conditions, and the following disclaimer,
//    without modification, immediately at the beginning of the file.
// 2. The name of the author may not be used to endorse or promote products
//    derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
// ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
// ARE DISCLAIMED. IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE FOR
// ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
// DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
// OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
// HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
// LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
// OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
// SUCH DAMAGE.
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use xc::models::network::PortRedirection;

///
pub(crate) struct PortForwardTable {
    db: ArcList<PortRedirection>,
}

impl PortForwardTable {
    pub(crate) fn new() -> PortForwardTable {
        PortForwardTable { db: ArcList::new() }
    }

    pub(crate) fn append_rule(&mut self, id: &str, rule: PortRedirection) {
        self.db.append(rule, id);
    }

    pub(crate) fn remove_rules(&mut self, id: &str) {
        self.db.remove_tagged(id);
    }

    pub(crate) fn all_rules(&self) -> Vec<PortRedirection> {
        let mut vector = Vec::new();
        self.db.foreach(|port| vector.push(port.clone()));
        vector
    }

    pub(crate) fn all_rules_with_id(&self, id: &str) -> Vec<PortRedirection> {
        let mut vector = Vec::new();
        self.db.foreach_tagged(id, |port| vector.push(port.clone()));
        vector
    }

    pub(crate) fn generate_rdr_rules(&self) -> String {
        let mut rules = String::new();
        self.db
            .foreach(|rule| writeln!(&mut rules, "{}", rule.to_pf_rule()).unwrap());
        rules
    }
}

/// Data structure purposely built to store the port forward rules. Since port forwarding are not
/// commutative, we need it to be order preserving. We also need to be able to query all rules of a
/// container in the right order, as well as remove the rules of a container efficiently.
pub(crate) struct ArcList<T> {
    head: Arc<RwLock<ArcListNode<T>>>,
    tail: Arc<RwLock<ArcListNode<T>>>,
    tokens: HashMap<String, Vec<Arc<RwLock<ArcListNode<T>>>>>,
}

struct ArcListNode<T> {
    prev: Option<Arc<RwLock<ArcListNode<T>>>>,
    next: Option<Arc<RwLock<ArcListNode<T>>>>,
    value: Option<T>,
}

impl<T> ArcList<T> {
    pub(crate) fn new() -> ArcList<T> {
        let head = ArcListNode {
            prev: None,
            next: None,
            value: None,
        };
        let tail = ArcListNode {
            prev: None,
            next: None,
            value: None,
        };

        let arc_head = Arc::new(RwLock::new(head));
        let arc_tail = Arc::new(RwLock::new(tail));

        arc_head.write().unwrap().next = Some(arc_tail.clone());
        arc_tail.write().unwrap().prev = Some(arc_head.clone());

        ArcList {
            head: arc_head,
            tail: arc_tail,
            tokens: HashMap::new(),
        }
    }

    pub(crate) fn append(&mut self, value: T, tag: &str) {
        let node = Arc::new(RwLock::new(ArcListNode {
            prev: { self.tail.clone().read().unwrap().prev.clone() },
            next: Some(self.tail.clone()),
            value: Some(value),
        }));

        self.tail
            .clone()
            .write()
            .unwrap()
            .prev
            .clone()
            .unwrap()
            .write()
            .unwrap()
            .next = Some(node.clone());

        self.tail.clone().write().unwrap().prev = Some(node.clone());

        match self.tokens.get_mut(&tag.to_string()) {
            None => {
                self.tokens.insert(tag.to_string(), vec![node]);
            }
            Some(v) => {
                v.push(node);
            }
        }
    }

    pub(crate) fn remove_tagged(&mut self, tag: &str) {
        if let Some(nodes) = self.tokens.remove(tag) {
            for node in nodes.into_iter() {
                self.remove_node(node);
            }
        }
    }

    fn remove_node(&self, node: Arc<RwLock<ArcListNode<T>>>) {
        let prev = { node.read().unwrap().prev.clone().unwrap() };
        let next = { node.read().unwrap().next.clone().unwrap() };
        let next2 = next.clone();
        {
            prev.write().unwrap().next = Some(next);
        }
        {
            next2.write().unwrap().prev = Some(prev);
        }
    }

    pub(crate) fn foreach<F>(&self, mut f: F)
    where
        F: FnMut(&T),
    {
        let mut node = self.head.clone();

        while node.clone().read().unwrap().next.is_some() {
            let xnode = node.clone();
            let borrowed = { xnode.read().unwrap() };
            if let Some(v) = borrowed.value.as_ref() {
                f(v);
            }
            node = borrowed.next.clone().unwrap();
        }
    }

    pub(crate) fn foreach_tagged<F>(&self, token: &str, mut f: F)
    where
        F: FnMut(&T),
    {
        if let Some(vector) = self.tokens.get(token) {
            vector.iter().for_each(|node| {
                let borrowed = node.read().unwrap();
                f(borrowed.value.as_ref().unwrap())
            });
        }
    }
}

/// Data structure purposely built to store the port forward rules. Since port forwarding are not
/// commutative, we need it to be order preserving. We also need to be able to query all rules of a
/// container in the right order, as well as remove the rules of a container efficiently.
#[allow(dead_code)]
pub(crate) struct List<T> {
    head: Rc<RefCell<ListNode<T>>>,
    tail: Rc<RefCell<ListNode<T>>>,
    tokens: HashMap<String, Vec<Rc<RefCell<ListNode<T>>>>>,
}

#[allow(dead_code)]
struct ListNode<T> {
    prev: Option<Rc<RefCell<ListNode<T>>>>,
    next: Option<Rc<RefCell<ListNode<T>>>>,
    value: Option<T>,
}

impl<T> List<T> {
    #[allow(dead_code)]
    pub(crate) fn new() -> List<T> {
        let head = ListNode {
            prev: None,
            next: None,
            value: None,
        };
        let tail = ListNode {
            prev: None,
            next: None,
            value: None,
        };

        let arc_head = Rc::new(RefCell::new(head));
        let arc_tail = Rc::new(RefCell::new(tail));

        arc_head.borrow_mut().next = Some(arc_tail.clone());
        arc_tail.borrow_mut().prev = Some(arc_head.clone());

        List {
            head: arc_head,
            tail: arc_tail,
            tokens: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn append(&mut self, value: T, tag: &str) {
        let node = Rc::new(RefCell::new(ListNode {
            prev: self.tail.clone().borrow().prev.clone(),
            next: Some(self.tail.clone()),
            value: Some(value),
        }));

        self.tail
            .clone()
            .borrow_mut()
            .prev
            .clone()
            .unwrap()
            .borrow_mut()
            .next = Some(node.clone());
        self.tail.clone().borrow_mut().prev = Some(node.clone());

        match self.tokens.get_mut(&tag.to_string()) {
            None => {
                self.tokens.insert(tag.to_string(), vec![node]);
            }
            Some(v) => {
                v.push(node);
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn remove_tagged(&mut self, tag: &str) {
        if let Some(nodes) = self.tokens.remove(tag) {
            for node in nodes.into_iter() {
                self.remove_node(node);
            }
        }
    }

    #[allow(dead_code)]
    fn remove_node(&self, node: Rc<RefCell<ListNode<T>>>) {
        let prev = node.borrow().prev.clone().unwrap();
        let next = node.borrow().next.clone().unwrap();
        let next2 = next.clone();
        prev.borrow_mut().next = Some(next);
        next2.borrow_mut().prev = Some(prev);
    }

    #[allow(dead_code)]
    pub(crate) fn foreach<F>(&self, mut f: F)
    where
        F: FnMut(&T),
    {
        let mut node = self.head.clone();

        while node.clone().borrow().next.is_some() {
            let xnode = node.clone();
            let borrowed = xnode.borrow();
            if let Some(v) = borrowed.value.as_ref() {
                f(v);
            }
            node = borrowed.next.clone().unwrap();
        }
    }

    #[allow(dead_code)]
    pub(crate) fn foreach_tagged<F>(&self, token: &str, mut f: F)
    where
        F: FnMut(&T),
    {
        if let Some(vector) = self.tokens.get(token) {
            vector.iter().for_each(|node| {
                let borrowed = node.borrow();
                f(borrowed.value.as_ref().unwrap())
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_ts() {
        let mut list = ArcList::<usize>::new();
        list.append(1, "1");
        list.append(2, "2");
        list.append(3, "2");
        list.append(4, "1");
        list.append(5, "3");
        list.append(6, "2");

        {
            let mut collected = Vec::new();
            list.foreach(|x| {
                collected.push(*x);
            });
            assert_eq!(collected, vec![1, 2, 3, 4, 5, 6]);
        }

        list.remove_tagged("1");

        {
            let mut collected = Vec::new();
            list.foreach_tagged("2", |x| {
                collected.push(*x);
            });
            assert_eq!(collected, vec![2, 3, 6]);
        }

        {
            let mut collected = Vec::new();
            list.foreach(|x| {
                collected.push(*x);
            });
            assert_eq!(collected, vec![2, 3, 5, 6]);
        }

        list.remove_tagged("3");

        {
            let mut collected = Vec::new();
            list.foreach(|x| {
                collected.push(*x);
            });
            assert_eq!(collected, vec![2, 3, 6]);
        }

        list.append(4, "2");
        list.append(5, "3");
        list.remove_tagged("2");

        {
            let mut collected = Vec::new();
            list.foreach(|x| {
                collected.push(*x);
            });
            assert_eq!(collected, vec![5]);
        }
    }

    #[test]
    fn test_list() {
        let mut list = List::<usize>::new();
        list.append(1, "1");
        list.append(2, "2");
        list.append(3, "2");
        list.append(4, "1");
        list.append(5, "3");
        list.append(6, "2");

        {
            let mut collected = Vec::new();
            list.foreach(|x| {
                collected.push(*x);
            });
            assert_eq!(collected, vec![1, 2, 3, 4, 5, 6]);
        }

        list.remove_tagged("1");

        {
            let mut collected = Vec::new();
            list.foreach_tagged("2", |x| {
                collected.push(*x);
            });
            assert_eq!(collected, vec![2, 3, 6]);
        }

        {
            let mut collected = Vec::new();
            list.foreach(|x| {
                collected.push(*x);
            });
            assert_eq!(collected, vec![2, 3, 5, 6]);
        }

        list.remove_tagged("3");

        {
            let mut collected = Vec::new();
            list.foreach(|x| {
                collected.push(*x);
            });
            assert_eq!(collected, vec![2, 3, 6]);
        }

        list.append(4, "2");
        list.append(5, "3");
        list.remove_tagged("2");

        {
            let mut collected = Vec::new();
            list.foreach(|x| {
                collected.push(*x);
            });
            assert_eq!(collected, vec![5]);
        }
    }
}

/*
   Copyright 2017 Takashi Ogura

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
 */
//! # Load [URDF](http://wiki.ros.org/urdf) format and create `k::LinkTree`
//!
extern crate nalgebra as na;
extern crate urdf_rs;

use std::collections::HashMap;
use std::path::Path;
use na::Real;

use links::*;
use idtree_links::*;
use idtree::*;
use rctree::*;
use rctree_links::*;
use joints::*;

/// Returns nalgebra::Unit<nalgebra::Vector3> from f64 array
pub fn axis_from<T>(array3: [f64; 3]) -> na::Unit<na::Vector3<T>>
where
    T: Real,
{
    na::Unit::<_>::new_normalize(na::Vector3::new(
        na::convert(array3[0]),
        na::convert(array3[1]),
        na::convert(array3[2]),
    ))
}

/// Returns nalgebra::UnitQuaternion from f64 array
pub fn quaternion_from<T>(array3: &[f64; 3]) -> na::UnitQuaternion<T>
where
    T: Real,
{
    na::convert(na::UnitQuaternion::from_euler_angles(
        array3[0],
        array3[1],
        array3[2],
    ))
}

/// Returns nalgebra::Translation3 from f64 array
pub fn translation_from<T>(array3: &[f64; 3]) -> na::Translation3<T>
where
    T: Real,
{
    na::convert(na::Translation3::new(array3[0], array3[1], array3[2]))
}

impl<T> Link<T>
where
    T: Real,
{
    pub fn from_urdf_joint(joint: &urdf_rs::Joint) -> Link<T> {
        let limit = if (joint.limit.upper - joint.limit.lower) == 0.0 {
            None
        } else {
            Some(Range::new(
                na::convert(joint.limit.lower),
                na::convert(joint.limit.upper),
            ))
        };
        LinkBuilder::<T>::new()
            .joint(
                &joint.name,
                match joint.joint_type {
                    urdf_rs::JointType::Revolute |
                    urdf_rs::JointType::Continuous => {
                        JointType::Rotational { axis: axis_from(joint.axis.xyz) }
                    }
                    urdf_rs::JointType::Prismatic => JointType::Linear {
                        axis: axis_from(joint.axis.xyz),
                    },
                    _ => JointType::Fixed,
                },
                limit,
            )
            .name(&joint.child.link)
            .rotation(quaternion_from(&joint.origin.rpy))
            .translation(translation_from(&joint.origin.xyz))
            .finalize()
    }
}

fn get_root_link_name(robot: &urdf_rs::Robot) -> String {
    let mut child_joint_map = HashMap::<&str, &urdf_rs::Joint>::new();
    for j in &robot.joints {
        if let Some(old) = child_joint_map.insert(&j.child.link, j) {
            warn!("old {:?} found", old);
        }
    }
    let mut parent_link_name: &str = &robot.links[0].name;
    while let Some(joint) = child_joint_map.get(&parent_link_name) {
        parent_link_name = &joint.parent.link;
    }
    parent_link_name.to_string()
}

/// Convert from URDF robot model
pub trait FromUrdf {
    fn from_urdf_robot(robot: &urdf_rs::Robot) -> Self;
    fn from_urdf_file<T, P>(path: P) -> Result<Self, urdf_rs::UrdfError>
    where
        Self: ::std::marker::Sized,
        P: AsRef<Path>,
    {
        Ok(Self::from_urdf_robot(&urdf_rs::read_file(path)?))
    }
}

impl<T> FromUrdf for RcLinkTree<T>
where
    T: Real,
{
    /// Create `RcLinkTree` from `urdf_rs::Robot`
    fn from_urdf_robot(robot: &urdf_rs::Robot) -> Self {
        let root_name = get_root_link_name(robot);
        let mut ref_nodes = Vec::new();
        let mut child_ref_map = HashMap::new();
        let mut parent_ref_map = HashMap::<&String, Vec<RcLinkNode<T>>>::new();
        let root_node = create_ref_node(
            LinkBuilder::<T>::new()
                .joint("root", JointType::Fixed, None)
                .name(&root_name)
                .finalize(),
        );
        for j in &robot.joints {
            let node = create_ref_node(Link::from_urdf_joint(j));
            child_ref_map.insert(&j.child.link, node.clone());
            if parent_ref_map.get(&j.parent.link).is_some() {
                parent_ref_map.get_mut(&j.parent.link).unwrap().push(
                    node.clone(),
                );
            } else {
                parent_ref_map.insert(&j.parent.link, vec![node.clone()]);
            }
            ref_nodes.push(node);
        }
        for l in &robot.links {
            info!("link={}", l.name);
            if let Some(parent_node) = child_ref_map.get(&l.name) {
                if let Some(child_nodes) = parent_ref_map.get(&l.name) {
                    for child_node in child_nodes.iter() {
                        info!(
                            "set paremt = {}, child = {}",
                            parent_node.borrow().data.get_joint_name(),
                            child_node.borrow().data.get_joint_name()
                        );
                        set_parent_child(parent_node, child_node);
                    }
                }
            }
        }
        // set root as parent of root joint nodes
        let root_joint_nodes = ref_nodes.iter().filter_map(
            |ref_node| match ref_node.borrow().parent {
                None => Some(ref_node),
                Some(_) => None,
            },
        );
        for rjn in root_joint_nodes {
            set_parent_child(&root_node, rjn);
        }
        // create root node..
        RcLinkTree::new(&robot.name, root_node)
    }
}

impl<T> FromUrdf for IdLinkTree<T>
where
    T: Real,
{
    /// Create `IdLinkTree` from `urdf_rs::Robot`
    fn from_urdf_robot(robot: &urdf_rs::Robot) -> Self {
        let root_name = get_root_link_name(robot);
        let mut nodes = Vec::new();
        let mut child_ref_map = HashMap::new();
        let mut parent_ref_map = HashMap::<_, Vec<_>>::new();
        let mut tree = IdTree::new();
        let root_node = tree.create_node(
            LinkBuilder::<T>::new()
                .joint("root", JointType::Fixed, None)
                .name(&root_name)
                .finalize(),
        );
        for j in &robot.joints {
            let node = tree.create_node(Link::from_urdf_joint(j));
            child_ref_map.insert(&j.child.link, node.clone());
            if parent_ref_map.get(&j.parent.link).is_some() {
                parent_ref_map.get_mut(&j.parent.link).unwrap().push(
                    node.clone(),
                );
            } else {
                parent_ref_map.insert(&j.parent.link, vec![node.clone()]);
            }
            nodes.push(node);
        }
        for l in &robot.links {
            info!("link={}", l.name);
            if let Some(parent_node) = child_ref_map.get(&l.name) {
                if let Some(child_nodes) = parent_ref_map.get(&l.name) {
                    for child_node in child_nodes.iter() {
                        info!(
                            "set paremt = {}, child = {}",
                            tree.get(parent_node).data.get_joint_name(),
                            tree.get(child_node).data.get_joint_name()
                        );
                        tree.set_parent_child(parent_node, child_node);
                    }
                }
            }
        }
        let root_node_ids = nodes
            .into_iter()
            .filter_map(|ref_node| match tree.get(&ref_node).parent {
                None => Some(ref_node.clone()),
                Some(_) => None,
            })
            .collect::<Vec<_>>();
        for id in root_node_ids {
            tree.set_parent_child(&root_node, &id);
        }
        IdLinkTree::new(&robot.name, tree)
    }
}


#[test]
fn test_tree() {
    let robo = urdf_rs::read_file("urdf/sample.urdf").unwrap();
    assert_eq!(robo.name, "robo");
    assert_eq!(robo.links.len(), 1 + 6 + 6);

    let tree = RcLinkTree::<f32>::from_urdf_robot(&robo);
    assert_eq!(tree.iter_link().map(|_| {}).count(), 13);
}

#[test]
fn test_tree_from_file() {
    let tree = RcLinkTree::<f32>::from_urdf_file::<f32, _>("urdf/sample.urdf").unwrap();
    assert_eq!(tree.dof(), 12);
    let names = tree.iter_link()
        .map(|link| link.get_joint_name().to_string())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 13);
    println!("{}", names[0]);
    assert_eq!(names[0], "root");
    assert_eq!(names[1], "l_shoulder_yaw");
}

#[test]
fn test_idtree_from_file() {
    let tree = IdLinkTree::<f32>::from_urdf_file::<f32, _>("urdf/sample.urdf").unwrap();
    assert_eq!(tree.dof(), 12);
    let names = tree.iter()
        .map(|node| node.data.get_joint_name())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 13);
    println!("{}", names[0]);
    println!("{}", names[1]);
    assert_eq!(names[0], "root");
    assert_eq!(names[1], "l_shoulder_yaw");
}

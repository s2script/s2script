//! Projection compatibility is independent of callback names and mutation rights.
use super::instance::Signature as Abi;

pub(crate) fn compatible(a: &Abi, b: &Abi) -> bool {
    fn position(a:&Abi,b:&Abi,x:&super::instance::Position,y:&super::instance::Position)->bool {
        if x.native!=y.native || x.ownership!=y.ownership || x.projection.version!=y.projection.version
            || !codec_compatible(&x.projection.id,&y.projection.id) {return false;}
        match (x.instance,y.instance) {
            (None,None)=>true,
            (Some(i),Some(j))=>a.instances.get(i).zip(b.instances.get(j)).is_some_and(|(x,y)|
                x.codec_id==y.codec_id && x.codec_version==y.codec_version && x.kind.is_none() && y.kind.is_none()
                && x.record.extent==y.record.extent && x.record.alignment==y.record.alignment
                && x.record.fields.len()==y.record.fields.len() && x.record.fields.iter().zip(&y.record.fields)
                    .all(|(x,y)|x.offset==y.offset && x.storage==y.storage)),
            _=>false,
        }
    }
    // Scratch selectors share one per-dispatch overlay: two declaring bindings must agree.
    // A binding without scratch never addresses those selectors.
    a.fingerprint == b.fingerprint && a.member_receiver == b.member_receiver
        && (a.scratch.is_empty() || b.scratch.is_empty() || a.scratch == b.scratch)
        && match (&a.receiver,&b.receiver) {(None,None)=>true,(Some(x),Some(y))=>position(a,b,x,y),_=>false}
        && a.parameters.len()==b.parameters.len()
        && a.parameters.iter().zip(&b.parameters).all(|(x,y)|position(a,b,x,y))
        && position(a,b,&a.returns,&b.returns)
}

fn codec_compatible(a: &str, b: &str) -> bool {
    a == b || (EntityProjection::parse(a).is_some() && EntityProjection::parse(b).is_some())
}
use crate::v8host::S2FunctionValue;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EntityReference {
    pub index: i32,
    pub id: u64,
}
#[derive(Clone)]
pub(crate) enum ProjectedValue {
    Scalar(S2FunctionValue),
    Copied(super::copied::Owned),
    Entity {
        reference: Option<EntityReference>,
        nullable: bool,
    },
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntityProjection {
    Strict,
    Nullable,
}
impl EntityProjection {
    pub(crate) fn parse(id: &str) -> Option<Self> {
        match id {
            "entity" => Some(Self::Strict),
            "entity?" => Some(Self::Nullable),
            _ => None,
        }
    }
    pub(crate) fn request(self) -> S2FunctionValue {
        S2FunctionValue {
            kind: 8,
            flags: if self == Self::Nullable { 2 } else { 1 },
            reserved: 0,
            aux: 0,
            bits: 0,
        }
    }
    pub(crate) fn value(
        self,
        reference: Option<EntityReference>,
    ) -> Result<ProjectedValue, String> {
        let reference =
            reference.filter(|r| crate::entity_live::engine_serial_for(r.index, r.id).is_some());
        if reference.is_none() && self == Self::Strict {
            return Err("strict entity projection is null or stale".into());
        }
        Ok(ProjectedValue::Entity {
            reference,
            nullable: self == Self::Nullable,
        })
    }
}
pub(crate) fn request(native: &str, projection: &str) -> Result<S2FunctionValue, String> {
    if let Some(entity) = EntityProjection::parse(projection) {
        if native != "ptr" {
            return Err("entity projection requires pointer ABI".into());
        }
        return Ok(entity.request());
    }
    if let Some(flags) = super::copied::flag(projection) {
        if native != "ptr" {
            return Err("copied projection requires pointer ABI".into());
        }
        return Ok(S2FunctionValue {
            kind: 8,
            flags,
            reserved: 0,
            aux: 0,
            bits: 0,
        });
    }
    let mut out = super::runtime::blank();
    out.kind = super::runtime::kind(native)?;
    Ok(out)
}
pub(crate) fn encode(value: ProjectedValue) -> Result<S2FunctionValue, String> {
    match value {
        ProjectedValue::Scalar(value) => Ok(value),
        ProjectedValue::Copied(_) => Err("copied value requires sidecar operation".into()),
        ProjectedValue::Entity {
            reference,
            nullable,
        } => {
            let projection = if nullable {
                EntityProjection::Nullable
            } else {
                EntityProjection::Strict
            };
            let mut out = projection.request();
            let live = reference.and_then(|r| {
                crate::entity_live::engine_serial_for(r.index, r.id).map(|serial| (r.index, serial))
            });
            match live {
                Some((index, serial)) => {
                    out.aux = index as u32;
                    out.bits = serial as u32 as u64;
                }
                None if nullable => out.aux = u32::MAX,
                None => return Err("strict entity projection is null or stale".into()),
            }
            Ok(out)
        }
    }
}
pub(crate) fn adopt(
    value: S2FunctionValue,
    projection: EntityProjection,
) -> Result<ProjectedValue, String> {
    if value.kind != 8
        || value.flags != projection.request().flags
        || value.reserved != 0
        || value.bits > u32::MAX as u64
    {
        return Err("invalid entity identity transport".into());
    }
    let reference = if value.aux == u32::MAX {
        if value.bits != 0 || projection != EntityProjection::Nullable {
            return Err("invalid entity null transport".into());
        }
        None
    } else {
        if value.aux > i32::MAX as u32 {
            return Err("entity index out of range".into());
        }
        crate::entity_live::adopt(value.aux as i32, value.bits as u32 as i32).map(|id| {
            EntityReference {
                index: value.aux as i32,
                id,
            }
        })
    };
    projection.value(reference)
}
pub(crate) fn decode(
    value: S2FunctionValue,
    native: &str,
    projection: &str,
) -> Result<ProjectedValue, String> {
    if let Some(entity) = EntityProjection::parse(projection) {
        return adopt(value, entity);
    }
    if value.kind != super::runtime::kind(native)?
        || value.flags != 0
        || value.reserved != 0
        || value.aux != 0
    {
        return Err("invalid scalar projection transport".into());
    }
    Ok(ProjectedValue::Scalar(value))
}

#[cfg(test)]
mod entity_tests {
    use super::*;
    #[test]
    fn entity_identity_adoption_is_books_gated_and_nullable_is_binding_local() {
        let strict = EntityProjection::Strict;
        let nullable = EntityProjection::Nullable;
        let id = crate::entity_live::on_created(901, 72);
        let value = ProjectedValue::Entity {
            reference: Some(EntityReference { index: 901, id }),
            nullable: false,
        };
        let wire = encode(value.clone()).unwrap();
        assert_eq!(
            (wire.kind, wire.flags, wire.aux, wire.bits),
            (8, 1, 901, 72)
        );
        assert!(matches!(
            adopt(wire, strict).unwrap(),
            ProjectedValue::Entity {
                reference: Some(_),
                ..
            }
        ));
        crate::entity_live::on_deleted(901, 72);
        assert!(encode(value.clone()).is_err());
        assert!(adopt(wire, strict).is_err());
        let mut nullable_wire = wire;
        nullable_wire.flags = 2;
        assert!(matches!(
            adopt(nullable_wire, nullable).unwrap(),
            ProjectedValue::Entity {
                reference: None,
                ..
            }
        ));
        let replacement = crate::entity_live::on_created(901, 73);
        assert_ne!(id, replacement);
        assert!(encode(value.clone()).is_err());
        let mut malformed = nullable.request();
        malformed.aux = u32::MAX;
        malformed.bits = 1;
        assert!(adopt(malformed, nullable).is_err());
        crate::entity_live::on_deleted(901, 73);
    }
}

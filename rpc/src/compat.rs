pub trait ToPrimitive<Out> {
    fn to_primitive(self) -> Out;
}

impl ToPrimitive<primitive_types::H160> for alloy::primitives::Address {
    fn to_primitive(self) -> primitive_types::H160 {
        let alloy::primitives::Address(alloy::primitives::FixedBytes(arr)) = self;
        primitive_types::H160(arr)
    }
}

impl ToPrimitive<primitive_types::H256> for alloy::primitives::B256 {
    fn to_primitive(self) -> primitive_types::H256 {
        let alloy::primitives::FixedBytes(arr) = self;
        primitive_types::H256(arr)
    }
}

impl ToPrimitive<[primitive_types::U256; 8]> for alloy::primitives::Bloom {
    fn to_primitive(self) -> [primitive_types::U256; 8] {
        let alloy::primitives::Bloom(alloy::primitives::FixedBytes(src)) = self;
        // have      u8 * 256
        // want    U256 * 8
        // (no unsafe, no unstable)
        let mut chunks = src.chunks_exact(32);
        let dst = core::array::from_fn(|_ix| {
            // This is a bit spicy because we're going from an uninterpeted array of bytes
            // to wide integers, but we trust this `From` impl to do the right thing
            primitive_types::U256::from(<[u8; 32]>::try_from(chunks.next().unwrap()).unwrap())
        });
        assert_eq!(chunks.len(), 0);
        dst
    }
}

impl ToPrimitive<primitive_types::U256> for alloy::primitives::U256 {
    fn to_primitive(self) -> primitive_types::U256 {
        primitive_types::U256(self.into_limbs())
    }
}

impl ToPrimitive<Vec<Vec<u8>>> for Vec<alloy::primitives::Bytes> {
    fn to_primitive(self) -> Vec<Vec<u8>> {
        self.into_iter().map(|x| x.to_vec()).collect()
    }
}

pub trait ToAlloy<Out> {
    fn to_alloy(self) -> Out;
}

impl ToAlloy<alloy::primitives::Address> for primitive_types::H160 {
    fn to_alloy(self) -> alloy::primitives::Address {
        let primitive_types::H160(arr) = self;
        alloy::primitives::Address(alloy::primitives::FixedBytes(arr))
    }
}

impl ToAlloy<alloy::primitives::StorageKey> for primitive_types::H256 {
    fn to_alloy(self) -> alloy::primitives::StorageKey {
        let primitive_types::H256(arr) = self;
        alloy::primitives::FixedBytes(arr)
    }
}

#[test]
fn bloom() {
    let _did_not_panic = alloy::primitives::Bloom::ZERO.to_primitive();
}

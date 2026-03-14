use super::*;

#[test]
fn test_one_or_many_one() {
    let mut v = OneOrMany::One(1);

    assert_eq!(v.clone().into_vec(), vec![1]);
    assert_eq!(v.as_slice(), &[1]);
    assert_eq!(v, OneOrMany::One(1));
    assert_eq!(format!("{v:?}"), "1");
    assert_eq!(OneOrMany::<()>::default(), OneOrMany::Many(vec![]));
    assert_eq!(v.first(), Some(&1));
    assert_eq!(v.first_mut(), Some(&mut 1));
    assert_eq!(OneOrMany::from_iter(vec![1]), OneOrMany::One(1));
    assert_eq!(v.clone().into_iter().collect::<Vec<_>>(), vec![1]);
    assert_eq!(OneOrMany::from(1), OneOrMany::One(1));
    assert_eq!(OneOrMany::from(vec![1]), OneOrMany::One(1));
    assert_eq!(Vec::from(v), vec![1]);
}

#[test]
fn test_one_or_many_many() {
    let mut v = OneOrMany::Many(vec![1, 2, 3]);

    assert_eq!(v.clone().into_vec(), vec![1, 2, 3]);
    assert_eq!(v.as_slice(), &[1, 2, 3]);
    assert_eq!(v, OneOrMany::Many(vec![1, 2, 3]));
    assert_eq!(format!("{v:?}"), "[1, 2, 3]");
    assert_eq!(OneOrMany::<()>::default(), OneOrMany::Many(vec![]));
    assert_eq!(v.last(), Some(&3));
    assert_eq!(v.last_mut(), Some(&mut 3));
    assert_eq!(
        OneOrMany::from_iter(vec![1, 2, 3]),
        OneOrMany::Many(vec![1, 2, 3])
    );
    assert_eq!(v.clone().into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    assert_eq!(
        OneOrMany::from(vec![1, 2, 3]),
        OneOrMany::Many(vec![1, 2, 3])
    );
    assert_eq!(Vec::from(v), vec![1, 2, 3]);
}

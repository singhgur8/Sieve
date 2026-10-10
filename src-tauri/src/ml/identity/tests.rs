use super::*;

/// Deterministic generator (LCG + Box-Muller).
struct Rng(u64);

impl Rng {
    fn next_f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32 + 0.5) / (1u64 << 24) as f32
    }

    fn gauss(&mut self) -> f32 {
        let (u, v) = (self.next_f32(), self.next_f32());
        (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
    }

    fn unit(&mut self, dim: usize) -> Vec<f32> {
        let mut v: Vec<f32> = (0..dim).map(|_| self.gauss()).collect();
        embed::normalize(&mut v);
        v
    }
}

/// `base` blended with noise so that the expected cosine to `base` is about `sim`.
fn around(rng: &mut Rng, base: &[f32], sim: f32) -> Vec<f32> {
    let noise = rng.unit(base.len());
    let k = (1.0 - sim * sim).sqrt();
    let mut v: Vec<f32> = base.iter().zip(&noise).map(|(b, n)| sim * b + k * n).collect();
    embed::normalize(&mut v);
    v
}

fn face(image_id: ImageId, face_index: u32, vector: Vec<f32>, quality: f32, bbox: NormRect) -> StoredEmbedding {
    StoredEmbedding {
        image_id,
        face_index,
        model_version: EMBED_MODEL_VERSION.to_owned(),
        bbox,
        vector,
        quality,
        person_id: None,
    }
}

fn rect(x: f32, y: f32, h: f32) -> NormRect {
    NormRect { x, y, width: h * 0.75, height: h }
}

const DIM: usize = 64;

/// `ids` identities; photo i shows the identities in `cast(i)` (one face each).
fn shoot(
    seed: u64,
    ids: usize,
    photos: usize,
    sim: f32,
    cast: impl Fn(usize) -> Vec<usize>,
) -> (Vec<StoredEmbedding>, Vec<usize>) {
    let mut rng = Rng(seed);
    let bases: Vec<Vec<f32>> = (0..ids).map(|_| rng.unit(DIM)).collect();
    let (mut faces, mut labels) = (Vec::new(), Vec::new());
    for p in 0..photos {
        for (k, who) in cast(p).into_iter().enumerate() {
            let v = around(&mut rng, &bases[who], sim);
            faces.push(face(p as ImageId + 1, k as u32, v, 0.9, rect(0.2 + 0.3 * k as f32, 0.3, 0.2)));
            labels.push(who);
        }
    }
    (faces, labels)
}

/// person index per face (None = unassigned).
fn assignment(people: &[PersonDraft], faces: &[StoredEmbedding]) -> Vec<Option<usize>> {
    let mut map = HashMap::new();
    for (p, d) in people.iter().enumerate() {
        for &k in &d.faces {
            map.insert(k, p);
        }
    }
    faces.iter().map(|f| map.get(&(f.image_id, f.face_index)).copied()).collect()
}

#[test]
fn clusters_synthetic_identities_purely() {
    // 6 identities, 60 photos with 1-3 people each.
    let (faces, labels) = shoot(7, 6, 60, 0.7, |p| match p % 4 {
        0 => vec![p % 6],
        1 => vec![p % 6, (p + 1) % 6],
        2 => vec![(p / 2) % 6, (p / 2 + 2) % 6, (p / 2 + 4) % 6],
        _ => vec![(p + 3) % 6],
    });
    let people = cluster_people(&faces, &[]);
    assert_eq!(people.len(), 6, "one person per identity");
    let assign = assignment(&people, &faces);
    for (a, l) in assign.iter().zip(&labels) {
        assert!(a.is_some(), "every good face is assigned");
        let _ = l;
    }
    for p in 0..people.len() {
        let ls: BTreeSet<usize> = assign.iter().zip(&labels).filter(|(a, _)| **a == Some(p)).map(|(_, l)| *l).collect();
        assert_eq!(ls.len(), 1, "cluster {p} mixes identities {ls:?}");
    }
    // Centroids are unit vectors, samples are capped and best first.
    for d in &people {
        let n: f32 = d.centroid.iter().map(|x| x * x).sum();
        assert!((n - 1.0).abs() < 1e-3);
        assert!(d.samples.len() <= MAX_SAMPLES && !d.samples.is_empty());
        assert_eq!(d.suggested_role, PersonRole::Unknown);
    }
    // Deterministic.
    assert_eq!(cluster_people(&faces, &[]), people);
}

#[test]
fn never_puts_two_faces_of_one_photo_in_one_person() {
    // Look-alikes (twins): two identities whose bases are close, always photographed together.
    let mut rng = Rng(11);
    let a = rng.unit(DIM);
    let b = around(&mut rng, &a, 0.55);
    let mut faces = Vec::new();
    for p in 0..20 {
        faces.push(face(p + 1, 0, around(&mut rng, &a, 0.8), 0.9, rect(0.2, 0.3, 0.2)));
        faces.push(face(p + 1, 1, around(&mut rng, &b, 0.8), 0.9, rect(0.6, 0.3, 0.2)));
    }
    let people = cluster_people(&faces, &[]);
    for d in &people {
        let photos: BTreeSet<ImageId> = d.faces.iter().map(|f| f.0).collect();
        assert_eq!(photos.len(), d.faces.len(), "one face per photo per person");
    }
    assert_eq!(people.len(), 2);
}

#[test]
fn singletons_are_not_people_and_weak_faces_need_more_similarity() {
    let mut rng = Rng(5);
    let a = rng.unit(DIM);
    let guest = rng.unit(DIM);
    let mut faces: Vec<StoredEmbedding> =
        (0..8).map(|p| face(p + 1, 0, around(&mut rng, &a, 0.75), 0.9, rect(0.3, 0.3, 0.2))).collect();
    // A guest seen once.
    faces.push(face(20, 0, guest.clone(), 0.9, rect(0.3, 0.3, 0.2)));
    // A weak face that is only loosely similar (0.44 > ASSIGN_SIM, < ASSIGN_SIM + margin to the centroid).
    let mut loose = around(&mut rng, &a, 0.44);
    embed::normalize(&mut loose);
    faces.push(face(21, 0, loose.clone(), 0.2, rect(0.3, 0.3, 0.05)));
    let people = cluster_people(&faces, &[]);
    assert_eq!(people.len(), 1);
    let p = &people[0];
    assert!(!p.faces.contains(&(20, 0)), "a one-off guest is nobody");
    let s = dot(&p.centroid, &loose);
    if s < ASSIGN_SIM + WEAK_FACE_MARGIN {
        assert!(!p.faces.contains(&(21, 0)), "weak loose face stays out (sim {s})");
    }
    // The same vector as a good face joins.
    let mut faces2 = faces.clone();
    faces2.last_mut().unwrap().quality = 0.9;
    let p2 = cluster_people(&faces2, &[]);
    if s >= ASSIGN_SIM {
        assert!(p2[0].faces.contains(&(21, 0)));
    }
}

#[test]
fn split_clusters_of_one_person_merge() {
    // One identity whose faces fall into two looser modes (e.g. glasses on / off): leader
    // clustering may open two clusters, the centroid merge joins them.
    let mut rng = Rng(3);
    let a = rng.unit(DIM);
    let m1 = around(&mut rng, &a, 0.8);
    let m2 = around(&mut rng, &a, 0.8);
    let mut faces = Vec::new();
    for p in 0..30 {
        let m = if p % 2 == 0 { &m1 } else { &m2 };
        faces.push(face(p + 1, 0, around(&mut rng, m, 0.75), 0.9, rect(0.3, 0.3, 0.2)));
    }
    let people = cluster_people(&faces, &[]);
    assert_eq!(people.len(), 1, "{:?}", people.iter().map(|p| p.faces.len()).collect::<Vec<_>>());
    assert_eq!(people[0].faces.len(), 30);
}

#[test]
fn reclustering_continues_prior_people() {
    let (faces, _) = shoot(9, 4, 40, 0.7, |p| vec![p % 4, (p + 1) % 4]);
    let first = cluster_people(&faces, &[]);
    assert_eq!(first.len(), 4);
    let prior: Vec<PriorPerson> = first
        .iter()
        .enumerate()
        .map(|(k, d)| PriorPerson { id: 100 + k as PersonId, centroid: d.centroid.clone(), user_role: None })
        .collect();
    // More photos arrive (same people, new noise) and the order changes.
    let (mut more, _) = shoot(9, 4, 60, 0.7, |p| vec![(p + 2) % 4]);
    for f in &mut more {
        f.image_id += 1000;
    }
    let mut all = more;
    all.extend(faces.iter().cloned());
    let second = cluster_people(&all, &prior);
    assert_eq!(second.len(), 4);
    for d in &second {
        let id = d.prior_id.expect("every person continues");
        // The continued person holds the same old faces.
        let old = &first[(id - 100) as usize];
        assert!(old.faces.iter().all(|f| d.faces.contains(f)));
    }
    let ids: BTreeSet<_> = second.iter().map(|d| d.prior_id).collect();
    assert_eq!(ids.len(), 4, "no prior id reused");
    // A new stranger cluster does not steal an id.
    let unrelated = vec![PriorPerson { id: 7, centroid: Rng(99).unit(DIM), user_role: None }];
    assert!(cluster_people(&faces, &unrelated).iter().all(|d| d.prior_id.is_none()));
}

/// People drafts directly from per-photo casts: `cast[i]` = (person, box) for photo i+1.
fn drafts(n: usize, cast: &[Vec<(usize, NormRect)>]) -> (Vec<PersonDraft>, Vec<StoredEmbedding>) {
    let mut people: Vec<PersonDraft> = (0..n)
        .map(|_| PersonDraft {
            prior_id: None,
            suggested_role: PersonRole::Unknown,
            ask: false,
            faces: Vec::new(),
            samples: Vec::new(),
            centroid: Vec::new(),
        })
        .collect();
    let mut faces = Vec::new();
    for (i, photo) in cast.iter().enumerate() {
        for (k, &(p, b)) in photo.iter().enumerate() {
            let key = (i as ImageId + 1, k as u32);
            people[p].faces.push(key);
            faces.push(face(key.0, key.1, vec![1.0], 0.9, b));
        }
    }
    (people, faces)
}

const BIG_L: NormRect = NormRect { x: 0.25, y: 0.25, width: 0.15, height: 0.2 };
const BIG_R: NormRect = NormRect { x: 0.55, y: 0.25, width: 0.15, height: 0.2 };
const CENTER: NormRect = NormRect { x: 0.42, y: 0.3, width: 0.15, height: 0.2 };
const SMALL_EDGE: NormRect = NormRect { x: 0.92, y: 0.05, width: 0.04, height: 0.05 };
const SMALL_EDGE2: NormRect = NormRect { x: 0.02, y: 0.05, width: 0.04, height: 0.05 };

/// A wedding: bride 0, groom 1, mother 2, sister 3, bridesmaid 4, officiant 5 (always small at
/// the edge), guests 6..
fn wedding() -> Vec<Vec<(usize, NormRect)>> {
    let mut cast = Vec::new();
    for _ in 0..40 {
        cast.push(vec![(0, BIG_L), (1, BIG_R)]); // couple
    }
    for _ in 0..20 {
        cast.push(vec![(0, CENTER)]); // bride prep
    }
    for _ in 0..12 {
        cast.push(vec![(1, CENTER)]); // groom prep
    }
    for _ in 0..25 {
        cast.push(vec![(0, BIG_L), (4, BIG_R)]); // bride + bridesmaid
    }
    for _ in 0..10 {
        cast.push(vec![(0, BIG_L), (2, BIG_R)]); // bride + mother
    }
    for _ in 0..6 {
        cast.push(vec![(3, CENTER), (2, BIG_L)]); // sister + mother
    }
    for _ in 0..30 {
        cast.push(vec![(0, BIG_L), (1, BIG_R), (5, SMALL_EDGE)]); // ceremony with officiant
    }
    for _ in 0..80 {
        cast.push(vec![(5, SMALL_EDGE), (4, SMALL_EDGE2)]); // officiant + bridesmaid, small, at the edges
    }
    for g in 0..12 {
        cast.push(vec![(6 + g, CENTER)]); // guests, once each ...
        cast.push(vec![(6 + g, BIG_L), (6 + (g + 1) % 12, BIG_R)]); // ... and twice
    }
    cast
}

#[test]
fn main_pair_is_the_couple_and_recurring_people_are_asked() {
    let cast = wedding();
    let (mut people, faces) = drafts(18, &cast);
    let stats = role_stats(&people, &faces);
    assert_eq!(stats.together[&(0, 1)].0, 70);
    // The officiant + bridesmaid appear together more often (80 photos), but small and at the
    // edges: size / centrality keep them out of the pair.
    assert!(stats.together[&(4, 5)].0 > stats.together[&(0, 1)].0);
    let mains = main_subjects(&stats, ShootType::Wedding);
    assert_eq!(mains, vec![0, 1]);
    suggest_roles(&mut people, &faces, ShootType::Wedding);
    let roles: Vec<(PersonRole, bool)> = people.iter().map(|p| (p.suggested_role, p.ask)).collect();
    assert_eq!(roles[0], (PersonRole::Main, false));
    assert_eq!(roles[1], (PersonRole::Main, false));
    for (p, r) in roles.iter().enumerate().take(6).skip(2) {
        assert_eq!(*r, (PersonRole::Unknown, true), "recurring person {p} is asked about");
    }
    for (p, r) in roles.iter().enumerate().skip(6) {
        assert_eq!(*r, (PersonRole::Other, false), "guest {p} seen <= 3 times is not asked about");
    }
}

#[test]
fn portrait_has_one_main_subject() {
    let mut cast = Vec::new();
    for _ in 0..50 {
        cast.push(vec![(0, CENTER)]);
    }
    for _ in 0..10 {
        cast.push(vec![(0, BIG_L), (1, BIG_R)]); // with the partner / parent for a few frames
    }
    let (mut people, faces) = drafts(2, &cast);
    suggest_roles(&mut people, &faces, ShootType::Portrait);
    assert_eq!(people[0].suggested_role, PersonRole::Main);
    assert_eq!((people[1].suggested_role, people[1].ask), (PersonRole::Unknown, true));
    // A wedding-type shoot with one dominant subject and a rare companion: one main too.
    let (mut people, faces) = drafts(2, &cast);
    suggest_roles(&mut people, &faces, ShootType::Wedding);
    assert_eq!(people[0].suggested_role, PersonRole::Main);
    assert_ne!(people[1].suggested_role, PersonRole::Main, "10 of 60 photos is not a couple");
}

#[test]
fn single_subject_shoot_and_empty_inputs() {
    assert!(main_subjects(&RoleStats::default(), ShootType::Wedding).is_empty());
    let mut none: Vec<PersonDraft> = Vec::new();
    suggest_roles(&mut none, &[], ShootType::Wedding);
    assert!(cluster_people(&[], &[]).is_empty());
    // Seen in too few photos to be a main subject.
    let (mut people, faces) = drafts(1, &[vec![(0, CENTER)], vec![(0, CENTER)]]);
    suggest_roles(&mut people, &faces, ShootType::Portrait);
    assert_eq!(people[0].suggested_role, PersonRole::Other);
}

#[test]
fn asks_about_at_most_eight_people_most_photos_first() {
    let mut cast = Vec::new();
    for _ in 0..30 {
        cast.push(vec![(0, BIG_L), (1, BIG_R)]);
    }
    // 12 recurring people with 5..16 photos each.
    for p in 2..14 {
        for _ in 0..(p + 3) {
            cast.push(vec![(p, CENTER)]);
        }
    }
    let (mut people, faces) = drafts(14, &cast);
    suggest_roles(&mut people, &faces, ShootType::Event);
    let asked: Vec<usize> = (0..14).filter(|&p| people[p].ask).collect();
    assert_eq!(asked, (6..14).collect::<Vec<_>>(), "the 8 with the most photos");
    assert!((2..6).all(|p| people[p].suggested_role == PersonRole::Other));
}

/// Clustering cost at wedding scale: 6,000 faces of 512-d (pairwise same-person cosine ~0.56,
/// about the LFW p5), 300 recurring people + 1,500
/// one-off guests. Ignored (timing): `cargo test --lib ml::identity::tests::cluster_scale -- --ignored --nocapture`.
#[test]
#[ignore]
fn cluster_scale() {
    let mut rng = Rng(42);
    let bases: Vec<Vec<f32>> = (0..300).map(|_| rng.unit(512)).collect();
    let mut faces = Vec::new();
    let mut img = 0;
    for k in 0..4500 {
        img += 1;
        faces.push(face(img, 0, around(&mut rng, &bases[k % 300], 0.75), 0.8, rect(0.3, 0.3, 0.2)));
    }
    for _ in 0..1500 {
        img += 1;
        faces.push(face(img, 0, rng.unit(512), 0.8, rect(0.3, 0.3, 0.2)));
    }
    let t = std::time::Instant::now();
    let people = cluster_people(&faces, &[]);
    let el = t.elapsed().as_secs_f64();
    let mut roles = people.clone();
    suggest_roles(&mut roles, &faces, ShootType::Wedding);
    println!("6000 faces -> {} people in {:.2} s (+ roles {:.2} s)", people.len(), el, t.elapsed().as_secs_f64() - el);
    assert_eq!(people.len(), 300);
}

// ---------------------------------------------------------------------------
// Catalog round trip (no model needed)
// ---------------------------------------------------------------------------

fn catalog() -> Connection {
    let conn = crate::db::open_in_memory();
    conn.execute("INSERT INTO projects (id, name, shoot_type, created_at) VALUES (1, 'P', 'wedding', 0)", []).unwrap();
    conn.execute("INSERT INTO folders (id, path, added_at, project_id) VALUES (1, '/f', 0, 1)", []).unwrap();
    conn
}

fn add_image(conn: &Connection, id: ImageId, preview: &str, metrics: Option<&ImageMetrics>) {
    conn.execute(
        "INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms,
             imported_at, captured_at_ms, rating, pick)
         VALUES (?1, 1, '/f/' || ?1 || '.arw', ?1 || '.arw', 'arw', 'sony', 1, 0, 0, ?2, 0, 'unflagged')",
        params![id, 1_000_000 + id * 1000],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO thumbnails (image_id, status, path, preview_path, extracted_at) VALUES (?1, 'ready', 'x', ?2, 1)",
        params![id, preview],
    )
    .unwrap();
    if let Some(m) = metrics {
        conn.execute(
            "INSERT INTO image_analysis (image_id, status, model_version, analyzed_at, faces_json, metrics_json)
             VALUES (?1, 'done', 'test', 1, '[]', ?2)",
            params![id, serde_json::to_string(m).unwrap()],
        )
        .unwrap();
    }
}

fn metrics_with(faces: Vec<crate::ml::FaceMetrics>) -> ImageMetrics {
    ImageMetrics {
        width: 2048,
        height: 1365,
        faces,
        global_sharpness: 0.5,
        exposure: crate::ipc::types::ExposureStats {
            clipped_highlights_pct: 0.0,
            clipped_shadows_pct: 0.0,
            mean_luma: 0.5,
        },
        phash: 0,
        tiles: crate::ml::TileStats { p90: 0.5, p50: 0.4, textured: 1.0, anisotropy: 0.1 },
        highlights: Default::default(),
    }
}

fn face_metrics(bbox: NormRect, kps: Option<&[[f32; 2]; 5]>, w: f32, h: f32, score: f32) -> crate::ml::FaceMetrics {
    // Yaw proxy as in the analysis: nose offset along the eye line / inter-ocular distance.
    let (left, right, yaw, iod) = match kps {
        Some(k) => {
            let (ex, ey) = (k[1][0] - k[0][0], k[1][1] - k[0][1]);
            let iod = (ex * ex + ey * ey).sqrt().max(1e-3);
            let (mx, my) = ((k[0][0] + k[1][0]) / 2.0, (k[0][1] + k[1][1]) / 2.0);
            let yaw = ((k[2][0] - mx) * ex + (k[2][1] - my) * ey) / (iod * iod);
            (k[0], k[1], yaw, iod)
        }
        None => ([0.0, 0.0], [0.0, 0.0], 0.0, 40.0),
    };
    crate::ml::FaceMetrics {
        bbox,
        left_eye: crate::ipc::types::NormPoint { x: left[0] / w, y: left[1] / h },
        right_eye: crate::ipc::types::NormPoint { x: right[0] / w, y: right[1] / h },
        detection_score: score,
        ear: None,
        sharpness: 0.5,
        ear_left: None,
        ear_right: None,
        mouth_open: None,
        yaw,
        iod_px: iod,
        face_sharpness: 0.5,
        eye_texture: 5.0,
        anisotropy: 0.1,
        frontal: true,
        truncated: false,
        eye_open_prob: None,
        head_pitch: None,
        head_yaw: None,
        mesh_ear: None,
        face_luma: 0.5,
        mouth_width: None,
        blown: 0.0,
    }
}

#[test]
fn update_people_without_model_leaves_people_alone() {
    let mut conn = catalog();
    let cancel = AtomicBool::new(false);
    let out =
        update_people(&mut conn, 1, ShootType::Wedding, Path::new("/nonexistent"), &cancel, &mut |_, _| {}).unwrap();
    assert!(out.people.is_none());
    assert!(out.model_version.is_none());
    assert!(out.message.unwrap().contains("not installed"));
}

#[test]
fn plan_embeds_only_usable_faces_and_drops_stale_rows() {
    let conn = catalog();
    let good = face_metrics(rect(0.3, 0.3, 0.2), None, 2048.0, 1365.0, 0.9);
    let tiny = face_metrics(rect(0.1, 0.1, 0.01), None, 2048.0, 1365.0, 0.9);
    add_image(&conn, 1, "/p/1.jpg", Some(&metrics_with(vec![good.clone(), tiny])));
    add_image(&conn, 2, "/p/2.jpg", Some(&metrics_with(vec![good.clone()])));
    add_image(&conn, 3, "/p/3.jpg", None); // not analysed
    let v = vec![0.5f32; 4];
    // Image 2 face 0 is current; image 1 face 0 has a moved box; image 1 face 1 is junk now.
    target::upsert_embedding(&conn, 2, 0, EMBED_MODEL_VERSION, &good.bbox, &v, 0.9).unwrap();
    target::upsert_embedding(&conn, 1, 0, EMBED_MODEL_VERSION, &rect(0.5, 0.3, 0.2), &v, 0.9).unwrap();
    target::upsert_embedding(&conn, 1, 1, EMBED_MODEL_VERSION, &rect(0.1, 0.1, 0.01), &v, 0.9).unwrap();
    // Image 3 still has an old row from an earlier analysis.
    target::upsert_embedding(&conn, 3, 0, "old-model", &good.bbox, &v, 0.9).unwrap();
    let stored = target::project_embeddings(&conn, 1).unwrap();
    let plan = plan(&conn, 1, &stored).unwrap();
    assert_eq!(plan.work.len(), 1);
    assert_eq!(plan.work[0].image_id, 1);
    assert_eq!(plan.work[0].faces.len(), 1);
    assert_eq!(plan.work[0].faces[0].face_index, 0);
    assert_eq!(plan.stale, vec![(1, 1), (3, 0)]);
    assert_eq!(plan.valid.len(), 2);
}

#[test]
fn image_people_reads_roles_per_face() {
    let mut conn = catalog();
    let good = face_metrics(rect(0.3, 0.3, 0.2), None, 2048.0, 1365.0, 0.9);
    for id in 1..=3 {
        add_image(&conn, id, "/p.jpg", Some(&metrics_with(vec![good.clone(), good.clone()])));
        target::upsert_embedding(&conn, id, 0, EMBED_MODEL_VERSION, &good.bbox, &[1.0, 0.0], 0.9).unwrap();
        target::upsert_embedding(&conn, id, 1, EMBED_MODEL_VERSION, &good.bbox, &[0.0, 1.0], 0.9).unwrap();
    }
    let mk = |faces: Vec<(ImageId, u32)>, role| PersonDraft {
        prior_id: None,
        suggested_role: role,
        ask: false,
        samples: faces.clone(),
        faces,
        centroid: vec![1.0, 0.0],
    };
    let ids = target::replace_people(
        &mut conn,
        1,
        &[mk(vec![(1, 0), (2, 0), (3, 0)], PersonRole::Main), mk(vec![(1, 1), (3, 1)], PersonRole::Unknown)],
    )
    .unwrap();
    target::set_person_role(&conn, ids[1], Some(PersonRole::Important)).unwrap();
    let map = image_people(&conn, 1).unwrap();
    assert_eq!(map.len(), 3);
    assert_eq!(
        map[&1],
        vec![
            FacePerson { face_index: 0, person_id: ids[0], role: PersonRole::Main },
            FacePerson { face_index: 1, person_id: ids[1], role: PersonRole::Important },
        ]
    );
    assert_eq!(map[&2].len(), 1);
    // Prior people come back with their centroid and the user's answer.
    let prior = prior_people(&conn, 1).unwrap();
    assert_eq!(prior.len(), 2);
    assert_eq!(prior[1].user_role, Some(PersonRole::Important));
    assert_eq!(prior[0].centroid, vec![1.0, 0.0]);
}

// ---------------------------------------------------------------------------
// Real model evaluation (ignored: needs `w600k_r50.onnx` + `det_10g.onnx` in the models dir
// and a labelled face set in `test-data/lfw-subset/<Name>/*.jpg`)
//   cargo test --lib ml::identity::tests::real -- --ignored --nocapture
// ---------------------------------------------------------------------------

mod real {
    use super::*;
    use crate::raw::turbo;
    use std::time::Instant;

    fn models_dir() -> PathBuf {
        std::env::var_os("SIEVE_MODELS")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("models"))
    }

    fn data_dir() -> PathBuf {
        std::env::var_os("SIEVE_FACE_SET")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-data/lfw-subset"))
    }

    /// (identity, file) of the labelled set, sorted.
    fn labelled_files() -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        let mut dirs: Vec<_> = std::fs::read_dir(data_dir()).expect("face set").flatten().collect();
        dirs.sort_by_key(|d| d.file_name());
        for d in dirs {
            let name = d.file_name().to_string_lossy().into_owned();
            let mut files: Vec<_> = std::fs::read_dir(d.path()).unwrap().flatten().map(|f| f.path()).collect();
            files.sort();
            out.extend(files.into_iter().map(|f| (name.clone(), f)));
        }
        out
    }

    struct Face {
        label: Option<usize>,
        image: usize,
        vector: Vec<f32>,
        bbox: [f32; 4],
    }

    /// Embeds every face of every labelled photo; the labelled face is the one nearest the
    /// centre (LFW photos are centred on their subject). Returns faces, names, timings.
    type EmbeddedSet = (Vec<Face>, Vec<String>, Vec<(String, PathBuf)>, f64);

    fn embed_set(pipe: &mut FacePipeline) -> EmbeddedSet {
        let files = labelled_files();
        let mut names: Vec<String> = files.iter().map(|f| f.0.clone()).collect();
        names.dedup();
        let mut faces = Vec::new();
        let mut embed_ms = Vec::new();
        for (img, (name, path)) in files.iter().enumerate() {
            let bytes = std::fs::read(path).unwrap();
            let d = turbo::decode_rgb(&bytes, u32::MAX, 64_000_000).unwrap();
            let (w, h) = (d.width as usize, d.height as usize);
            let dets = pipe.embed_all(&d.pixels, w, h).unwrap();
            // Time the embedding alone.
            for (det, _) in &dets {
                let t = Instant::now();
                pipe.embedder.embed(&d.pixels, w, h, &det.kps).unwrap();
                embed_ms.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            let centre =
                |b: &[f32; 4]| ((b[0] + b[2]) / 2.0 - w as f32 / 2.0).hypot((b[1] + b[3]) / 2.0 - h as f32 / 2.0);
            let main = (0..dets.len()).min_by(|&a, &b| centre(&dets[a].0.bbox).total_cmp(&centre(&dets[b].0.bbox)));
            let label = names.iter().position(|n| n == name).unwrap();
            for (k, (det, v)) in dets.into_iter().enumerate() {
                faces.push(Face { label: (Some(k) == main).then_some(label), image: img, vector: v, bbox: det.bbox });
            }
        }
        embed_ms.sort_by(f64::total_cmp);
        let median = embed_ms.get(embed_ms.len() / 2).copied().unwrap_or(0.0);
        (faces, names, files, median)
    }

    struct Eval {
        purity: f64,
        clusters: usize,
        split_identities: usize,
        merged_clusters: usize,
        unassigned: usize,
    }

    fn evaluate(faces: &[Face], names: &[String], params: &ClusterParams) -> Eval {
        let stored: Vec<StoredEmbedding> = faces
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let h = f.bbox[3] - f.bbox[1];
                // LFW is 250 px: the centred faces are ~100 px, background faces smaller.
                let q = ((h - 24.0) / 48.0).clamp(0.0, 1.0);
                face(f.image as ImageId + 1, i as u32, f.vector.clone(), q, rect(0.0, 0.0, 0.1))
            })
            .filter(|f| f.quality >= EMBED_MIN_QUALITY)
            .collect();
        let people = cluster_people_with(&stored, &[], params);
        let mut person_of: HashMap<u32, usize> = HashMap::new();
        for (p, d) in people.iter().enumerate() {
            for &(_, fi) in &d.faces {
                person_of.insert(fi, p);
            }
        }
        let labelled: Vec<(usize, Option<usize>)> = faces
            .iter()
            .enumerate()
            .filter_map(|(i, f)| f.label.map(|l| (l, person_of.get(&(i as u32)).copied())))
            .collect();
        let mut table: BTreeMap<usize, BTreeMap<usize, usize>> = BTreeMap::new(); // cluster -> label -> n
        for &(l, p) in &labelled {
            if let Some(p) = p {
                *table.entry(p).or_default().entry(l).or_default() += 1;
            }
        }
        let assigned: usize = table.values().flat_map(|m| m.values()).sum();
        let majority: usize = table.values().map(|m| m.values().max().copied().unwrap_or(0)).sum();
        let merged = table.values().filter(|m| m.len() > 1).count();
        let mut clusters_per_label = vec![BTreeSet::new(); names.len()];
        for (&p, m) in &table {
            for (&l, &n) in m {
                // a label "lives" in a cluster when >= 2 of its faces are there
                if n >= 2 {
                    clusters_per_label[l].insert(p);
                }
            }
        }
        Eval {
            purity: if assigned > 0 { majority as f64 / assigned as f64 } else { 0.0 },
            clusters: people.len(),
            split_identities: clusters_per_label.iter().filter(|s| s.len() > 1).count(),
            merged_clusters: merged,
            unassigned: labelled.len() - assigned,
        }
    }

    #[test]
    #[ignore]
    fn lfw_purity_thresholds_and_timing() {
        let t = Instant::now();
        let mut pipe = FacePipeline::load(&models_dir()).expect("models");
        println!("load {:.0} ms, embedder on {:?}", t.elapsed().as_secs_f64() * 1000.0, pipe.embedder.provider);
        println!("{}", pipe.embedder.describe());
        let (faces, names, files, embed_median) = embed_set(&mut pipe);
        let labelled = faces.iter().filter(|f| f.label.is_some()).count();
        println!(
            "{} photos, {} identities, {} faces ({} labelled, {} background); embedding median {:.1} ms/face",
            files.len(),
            names.len(),
            faces.len(),
            labelled,
            faces.len() - labelled,
            embed_median
        );
        // Similarity distributions of labelled faces.
        let lab: Vec<&Face> = faces.iter().filter(|f| f.label.is_some()).collect();
        let (mut same, mut diff) = (Vec::new(), Vec::new());
        for i in 0..lab.len() {
            for j in i + 1..lab.len() {
                let s = dot(&lab[i].vector, &lab[j].vector);
                if lab[i].label == lab[j].label {
                    same.push(s)
                } else {
                    diff.push(s)
                }
            }
        }
        let pct = |v: &mut Vec<f32>, q: f32| {
            v.sort_by(f32::total_cmp);
            v[((v.len() - 1) as f32 * q) as usize]
        };
        println!(
            "same-person cos: p1 {:.3} p5 {:.3} p50 {:.3} | different: p50 {:.3} p95 {:.3} p99 {:.3} max {:.3}",
            pct(&mut same, 0.01),
            pct(&mut same, 0.05),
            pct(&mut same, 0.5),
            pct(&mut diff, 0.5),
            pct(&mut diff, 0.95),
            pct(&mut diff, 0.99),
            pct(&mut diff, 1.0)
        );
        // Threshold sweep.
        for assign in [0.25f32, 0.3, 0.35, 0.4, 0.45, 0.5, 0.55] {
            let p = ClusterParams { assign_sim: assign, merge_sim: assign + 0.05, ..ClusterParams::default() };
            let e = evaluate(&faces, &names, &p);
            println!(
                "assign {:.2} merge {:.2}: purity {:.3}, {} clusters, {} identities split, {} clusters merged, {} labelled faces unassigned",
                assign, p.merge_sim, e.purity, e.clusters, e.split_identities, e.merged_clusters, e.unassigned
            );
        }
        let e = evaluate(&faces, &names, &ClusterParams::default());
        println!(
            "DEFAULT: purity {:.3}, {} clusters, split {}, merged {}, unassigned {}",
            e.purity, e.clusters, e.split_identities, e.merged_clusters, e.unassigned
        );
        assert!(e.purity >= 0.9, "purity {}", e.purity);
        assert!(e.split_identities <= 1 && e.merged_clusters == 0);

        // Near-duplicates: the same photo re-encoded / shifted / rescaled / brightened must stay
        // far above the assignment threshold.
        let mut worst: f32 = 1.0;
        for (name, path) in files.iter().step_by(10) {
            let bytes = std::fs::read(path).unwrap();
            let d = turbo::decode_rgb(&bytes, u32::MAX, 64_000_000).unwrap();
            let (w, h) = (d.width as usize, d.height as usize);
            let base = central(&mut pipe, &d.pixels, w, h);
            let mut variants: Vec<(&str, Vec<u8>, usize, usize)> = Vec::new();
            let q40 = turbo::encode_rgb(&d.pixels, w as u32, h as u32, 40).unwrap();
            let q40 = turbo::decode_rgb(&q40, u32::MAX, 64_000_000).unwrap();
            variants.push(("jpeg q40", q40.pixels, w, h));
            variants.push(("brighter", d.pixels.iter().map(|&v| (v as f32 * 1.25).min(255.0) as u8).collect(), w, h));
            let mut shifted = vec![0u8; w * h * 3];
            for y in 0..h {
                for x in 12..w {
                    let (s, o) = ((y * w + x - 12) * 3, (y * w + x) * 3);
                    shifted[o..o + 3].copy_from_slice(&d.pixels[s..s + 3]);
                }
            }
            variants.push(("shift 12px", shifted, w, h));
            let mut mirrored = vec![0u8; w * h * 3];
            for y in 0..h {
                for x in 0..w {
                    let (s, o) = ((y * w + (w - 1 - x)) * 3, (y * w + x) * 3);
                    mirrored[o..o + 3].copy_from_slice(&d.pixels[s..s + 3]);
                }
            }
            variants.push(("mirror", mirrored, w, h));
            let (hw, hh) = (w / 2, h / 2);
            let mut half = vec![0u8; hw * hh * 3];
            for y in 0..hh {
                for x in 0..hw {
                    for c in 0..3 {
                        let s: u32 = [(0, 0), (1, 0), (0, 1), (1, 1)]
                            .iter()
                            .map(|(dx, dy)| d.pixels[((2 * y + dy) * w + 2 * x + dx) * 3 + c] as u32)
                            .sum();
                        half[(y * hw + x) * 3 + c] = (s / 4) as u8;
                    }
                }
            }
            variants.push(("half size", half, hw, hh));
            let mut line = format!("{name}:");
            for (label, px, vw, vh) in variants {
                let v = central(&mut pipe, &px, vw, vh);
                if let (Some(a), Some(b)) = (&base, &v) {
                    let s = dot(a, b);
                    worst = worst.min(s);
                    line += &format!(" {label} {s:.3}");
                }
            }
            println!("{line}");
        }
        println!("near-duplicate worst cos {worst:.3} (assign threshold {ASSIGN_SIM})");
        assert!(worst > ASSIGN_SIM + 0.1);

        // Small faces (wide / group shots): every photo again at 1/3 size (subject ~33 px,
        // a weak face that may only join clusters formed by the good faces).
        let mut mixed: Vec<Face> = faces;
        let n_good = mixed.len();
        for (img, (name, path)) in files.iter().enumerate() {
            let d = turbo::decode_rgb(&std::fs::read(path).unwrap(), u32::MAX, 64_000_000).unwrap();
            let (w, h) = (d.width as usize, d.height as usize);
            let (sw, sh) = (w / 3, h / 3);
            let mut small = vec![0u8; sw * sh * 3];
            for y in 0..sh {
                for x in 0..sw {
                    for c in 0..3 {
                        let mut acc = 0u32;
                        for dy in 0..3 {
                            for dx in 0..3 {
                                acc += d.pixels[((3 * y + dy) * w + 3 * x + dx) * 3 + c] as u32;
                            }
                        }
                        small[(y * sw + x) * 3 + c] = (acc / 9) as u8;
                    }
                }
            }
            let off =
                |b: &[f32; 4]| ((b[0] + b[2]) / 2.0 - sw as f32 / 2.0).hypot((b[1] + b[3]) / 2.0 - sh as f32 / 2.0);
            if let Some((det, v)) = pipe
                .embed_all(&small, sw, sh)
                .unwrap()
                .into_iter()
                .min_by(|a, b| off(&a.0.bbox).total_cmp(&off(&b.0.bbox)))
            {
                mixed.push(Face {
                    label: names.iter().position(|n| n == name),
                    image: 1000 + img,
                    vector: v,
                    bbox: det.bbox,
                });
            }
        }
        let e = evaluate(&mixed, &names, &ClusterParams::default());
        println!(
            "with {} small faces added: purity {:.3}, {} clusters, split {}, merged {}, labelled faces unassigned {} (of {})",
            mixed.len() - n_good,
            e.purity,
            e.clusters,
            e.split_identities,
            e.merged_clusters,
            e.unassigned,
            mixed.iter().filter(|f| f.label.is_some()).count()
        );
        assert!(e.purity >= 0.9);
    }

    /// Embedding of the face nearest the image centre (the LFW subject).
    fn central(pipe: &mut FacePipeline, px: &[u8], w: usize, h: usize) -> Option<Vec<f32>> {
        let off = |b: &[f32; 4]| ((b[0] + b[2]) / 2.0 - w as f32 / 2.0).hypot((b[1] + b[3]) / 2.0 - h as f32 / 2.0);
        pipe.embed_all(px, w, h)
            .unwrap()
            .into_iter()
            .min_by(|a, b| off(&a.0.bbox).total_cmp(&off(&b.0.bbox)))
            .map(|(_, v)| v)
    }

    /// Pastes `src` (sw x sh RGB) scaled by `k` (nearest-neighbour + 2x2 smoothing is enough
    /// for this purpose) at (x0, y0) into `dst` (dw x dh).
    #[allow(clippy::too_many_arguments)]
    fn paste(dst: &mut [u8], dw: usize, dh: usize, src: &[u8], sw: usize, sh: usize, k: f32, x0: usize, y0: usize) {
        let (tw, th) = ((sw as f32 * k) as usize, (sh as f32 * k) as usize);
        for y in 0..th.min(dh.saturating_sub(y0)) {
            for x in 0..tw.min(dw.saturating_sub(x0)) {
                let (fx, fy) = (x as f32 / k, y as f32 / k);
                let (ix, iy) = ((fx as usize).min(sw - 2), (fy as usize).min(sh - 2));
                let (ax, ay) = (fx - ix as f32, fy - iy as f32);
                for c in 0..3 {
                    let p = |xx: usize, yy: usize| src[(yy * sw + xx) * 3 + c] as f32;
                    let v = p(ix, iy) * (1.0 - ax) * (1.0 - ay)
                        + p(ix + 1, iy) * ax * (1.0 - ay)
                        + p(ix, iy + 1) * (1.0 - ax) * ay
                        + p(ix + 1, iy + 1) * ax * ay;
                    dst[((y0 + y) * dw + x0 + x) * 3 + c] = v as u8;
                }
            }
        }
    }

    /// End to end on a synthetic "wedding" catalog: 2048 x 1365 previews composed of LFW
    /// photos (scaled x2.2), the couple = identities 0 + 1, a parent = 2 (with the bride),
    /// a group of 3 others, single shots of everyone. `SIEVE_E2E_PHOTOS` image rows (default
    /// 300; 2500 for the shoot-size timing) cycle over the distinct previews.
    #[test]
    #[ignore]
    fn end_to_end_main_pair_and_timing() {
        let files = labelled_files();
        let mut names: Vec<String> = files.iter().map(|f| f.0.clone()).collect();
        names.dedup();
        let by_name = |i: usize, k: usize| -> PathBuf {
            let list: Vec<&PathBuf> = files.iter().filter(|f| f.0 == names[i]).map(|f| &f.1).collect();
            list[k % list.len()].clone()
        };
        let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-data/identity-e2e");
        std::fs::create_dir_all(&out_dir).unwrap();
        // Scenes: list of identities per preview.
        let mut scenes: Vec<Vec<usize>> = Vec::new();
        for _ in 0..16 {
            scenes.push(vec![0, 1]);
        }
        for _ in 0..6 {
            scenes.push(vec![0]);
        }
        for _ in 0..5 {
            scenes.push(vec![1]);
        }
        for _ in 0..6 {
            scenes.push(vec![0, 2]);
        }
        for _ in 0..4 {
            scenes.push(vec![3, 4, 5]);
        }
        for i in 2..names.len() {
            scenes.push(vec![i]);
        }
        let (dw, dh) = (2048usize, 1365usize);
        let mut previews = Vec::new();
        let mut counters = vec![0usize; names.len()];
        // Per scene: (centre x, centre y, tolerance, identity) of each pasted LFW subject.
        let mut subjects: Vec<Vec<(f32, f32, f32, usize)>> = Vec::new();
        for (s, cast) in scenes.iter().enumerate() {
            let mut canvas = vec![90u8; dw * dh * 3];
            subjects.push(Vec::new());
            let n = cast.len();
            for (slot, &who) in cast.iter().enumerate() {
                let path = by_name(who, counters[who]);
                counters[who] += 1;
                let d = turbo::decode_rgb(&std::fs::read(path).unwrap(), u32::MAX, 64_000_000).unwrap();
                let k = 2.2;
                let tw = (d.width as f32 * k) as usize;
                let x0 = (dw / (n + 1)) * (slot + 1) - tw / 2;
                paste(&mut canvas, dw, dh, &d.pixels, d.width as usize, d.height as usize, k, x0, 200);
                let th = d.height as f32 * k;
                subjects[s].push((x0 as f32 + tw as f32 / 2.0, 200.0 + th / 2.0, 0.2 * tw as f32, who));
            }
            let jpg = turbo::encode_rgb(&canvas, dw as u32, dh as u32, 90).unwrap();
            let p = out_dir.join(format!("scene_{s:03}.jpg"));
            std::fs::write(&p, jpg).unwrap();
            previews.push(p);
        }
        // Catalog: metrics from SCRFD as the analysis would store them (largest first).
        let mut pipe = FacePipeline::load(&models_dir()).expect("models");
        let mut metrics = Vec::new();
        // Identity label per (scene, face index): the pasted subject whose centre the face is on.
        let mut face_label: HashMap<(usize, u32), usize> = HashMap::new();
        let (mut t_decode, mut t_all, mut n_faces) = (0.0, 0.0, 0usize);
        for (s, p) in previews.iter().enumerate() {
            let t = Instant::now();
            let d = turbo::decode_rgb(&std::fs::read(p).unwrap(), u32::MAX, 64_000_000).unwrap();
            t_decode += t.elapsed().as_secs_f64();
            let t = Instant::now();
            let mut dets: Vec<_> = pipe.embed_all(&d.pixels, dw, dh).unwrap().into_iter().map(|x| x.0).collect();
            t_all += t.elapsed().as_secs_f64();
            n_faces += dets.len();
            dets.sort_by(|a, b| (b.bbox[3] - b.bbox[1]).total_cmp(&(a.bbox[3] - a.bbox[1])));
            for (i, det) in dets.iter().enumerate() {
                let (cx, cy) = ((det.bbox[0] + det.bbox[2]) / 2.0, (det.bbox[1] + det.bbox[3]) / 2.0);
                if let Some(&(_, _, _, who)) =
                    subjects[s].iter().find(|(sx, sy, tol, _)| (cx - sx).hypot(cy - sy) < *tol)
                {
                    face_label.insert((s, i as u32), who);
                }
            }
            let faces = dets
                .iter()
                .map(|det| {
                    let b = det.bbox;
                    let bbox = NormRect {
                        x: b[0] / dw as f32,
                        y: b[1] / dh as f32,
                        width: (b[2] - b[0]) / dw as f32,
                        height: (b[3] - b[1]) / dh as f32,
                    };
                    face_metrics(bbox, Some(&det.kps), dw as f32, dh as f32, det.score)
                })
                .collect();
            metrics.push(metrics_with(faces));
        }
        let t = Instant::now();
        for p in &previews {
            pipe.embed_preview(p, &[]).unwrap();
        }
        let t_detect = t.elapsed().as_secs_f64() - t_decode;
        println!(
            "single thread: SCRFD incl. 2048 -> 640 resize {:.1} ms/photo",
            t_detect * 1000.0 / previews.len() as f64
        );
        drop(pipe);
        println!(
            "single thread, {} previews: decode {:.1} ms/photo, detect + embed {:.1} ms/photo ({:.2} faces/photo)",
            previews.len(),
            t_decode * 1000.0 / previews.len() as f64,
            t_all * 1000.0 / previews.len() as f64,
            n_faces as f64 / previews.len() as f64
        );
        let n_photos: usize = std::env::var("SIEVE_E2E_PHOTOS").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
        let dir = tempfile::tempdir().unwrap();
        let mut conn = crate::db::open(&dir.path().join("cat.sqlite")).unwrap();
        conn.execute("INSERT INTO projects (id, name, shoot_type, created_at) VALUES (1, 'P', 'wedding', 0)", [])
            .unwrap();
        conn.execute("INSERT INTO folders (id, path, added_at, project_id) VALUES (1, '/f', 0, 1)", []).unwrap();
        let mut scene_of = Vec::new();
        {
            let tx = conn.transaction().unwrap();
            for id in 1..=n_photos {
                let s = (id - 1) % previews.len();
                scene_of.push(s);
                add_image(&tx, id as ImageId, &previews[s].to_string_lossy(), Some(&metrics[s]));
            }
            tx.commit().unwrap();
        }
        let faces_total: usize = scene_of.iter().map(|&s| metrics[s].faces.len()).sum();
        let cancel = AtomicBool::new(false);
        let t = Instant::now();
        let out = update_people(&mut conn, 1, ShootType::Wedding, &models_dir(), &cancel, &mut |_, _| {}).unwrap();
        let first = t.elapsed().as_secs_f64();
        let people = out.people.clone().expect("people");
        println!(
            "{} photos, {} faces: embedded {} in {:.1} s ({:.1} ms/photo, {:.1} ms/face, {} threads)",
            n_photos,
            faces_total,
            out.embedded,
            first,
            first * 1000.0 / n_photos as f64,
            first * 1000.0 / out.embedded.max(1) as f64,
            std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_EMBED_THREADS)
        );
        let ids = target::replace_people(&mut conn, 1, &people).unwrap();
        // Which identity is each person? Majority over its labelled faces (None = only
        // background faces of the LFW photos).
        let label_of = |d: &PersonDraft| -> (Option<usize>, usize, usize) {
            let mut counts = vec![0usize; names.len()];
            for &(img, fi) in &d.faces {
                if let Some(&l) = face_label.get(&(scene_of[img as usize - 1], fi)) {
                    counts[l] += 1;
                }
            }
            let total: usize = counts.iter().sum();
            let (best, n) = counts.iter().enumerate().max_by_key(|x| x.1).map(|(i, &n)| (i, n)).unwrap();
            ((n > 0).then_some(best), n, total)
        };
        let (mut majority, mut labelled_assigned) = (0, 0);
        let mut persons_per_identity = vec![0usize; names.len()];
        for (d, id) in people.iter().zip(&ids) {
            let (l, n, total) = label_of(d);
            majority += n;
            labelled_assigned += total;
            if let Some(l) = l {
                persons_per_identity[l] += 1;
            }
            println!(
                "person {id}: {} faces, role {:?} ask {} -> {} ({n} of {total} labelled faces)",
                d.faces.len(),
                d.suggested_role,
                d.ask,
                l.map_or("background face", |l| names[l].as_str()),
            );
        }
        let labelled_total = scene_of
            .iter()
            .map(|&s| (0..metrics[s].faces.len() as u32).filter(|&i| face_label.contains_key(&(s, i))).count())
            .sum::<usize>();
        println!(
            "end-to-end purity {:.3} ({} of {} labelled faces assigned); identities split: {}",
            majority as f64 / labelled_assigned.max(1) as f64,
            labelled_assigned,
            labelled_total,
            persons_per_identity.iter().filter(|&&n| n > 1).count()
        );
        assert!(majority as f64 >= 0.9 * labelled_assigned as f64);
        let mains: BTreeSet<Option<usize>> =
            people.iter().filter(|d| d.suggested_role == PersonRole::Main).map(|d| label_of(d).0).collect();
        assert_eq!(mains, BTreeSet::from([Some(0), Some(1)]), "the couple is the main pair");
        let parent = people.iter().find(|d| label_of(d).0 == Some(2)).expect("parent found");
        assert!(parent.ask, "the parent is asked about");
        // Re-run: nothing to embed, same people continue.
        let t = Instant::now();
        let again = update_people(&mut conn, 1, ShootType::Wedding, &models_dir(), &cancel, &mut |_, _| {}).unwrap();
        println!("re-run (no new faces): {:.2} s", t.elapsed().as_secs_f64());
        assert_eq!(again.embedded, 0);
        let again = again.people.unwrap();
        assert_eq!(again.len(), people.len());
        assert!(again.iter().all(|d| d.prior_id.is_some_and(|p| ids.contains(&p))));
        let map = image_people(&conn, 1).unwrap();
        assert!(map.values().flatten().any(|f| f.role == PersonRole::Main));
        std::fs::remove_dir_all(&out_dir).unwrap();
    }
}

export type Maybe<T> = T | null;
export type InputMaybe<T> = Maybe<T>;
export type Exact<T extends { [key: string]: unknown }> = { [K in keyof T]: T[K] };
export type MakeOptional<T, K extends keyof T> = Omit<T, K> & { [SubKey in K]?: Maybe<T[SubKey]> };
export type MakeMaybe<T, K extends keyof T> = Omit<T, K> & { [SubKey in K]: Maybe<T[SubKey]> };
export type MakeEmpty<T extends { [key: string]: unknown }, K extends keyof T> = { [_ in K]?: never };
export type Incremental<T> = T | { [P in keyof T]?: P extends ' $fragmentName' | '__typename' ? T[P] : never };
/** All built-in and custom scalars, mapped to their actual values */
export type Scalars = {
  ID: { input: string; output: string; }
  String: { input: string; output: string; }
  Boolean: { input: boolean; output: boolean; }
  Int: { input: number; output: number; }
  Float: { input: number; output: number; }
  DateTime: { input: string; output: string; }
  JSON: { input: unknown; output: unknown; }
  /** Money in a currency. */
  Money: { input: unknown; output: unknown; }
  URL: { input: string; output: string; }
};

export type Address = {
  __typename?: 'Address';
  city?: Maybe<Scalars['String']['output']>;
  country: CountryCode;
  line1: Scalars['String']['output'];
  line2?: Maybe<Scalars['String']['output']>;
};

export type Cat = Pet & {
  __typename?: 'Cat';
  id: Scalars['ID']['output'];
  lives: Scalars['Int']['output'];
};

export type Collection = Named & Node & {
  __typename?: 'Collection';
  id: Scalars['ID']['output'];
  name: Scalars['String']['output'];
  products: Array<Product>;
};

export const CountryCode = {
  Fr: 'FR',
  Us: 'US'
} as const;

export type CountryCode = typeof CountryCode[keyof typeof CountryCode];
export type Dog = Pet & {
  __typename?: 'Dog';
  barks: Scalars['Boolean']['output'];
  id: Scalars['ID']['output'];
};

export type Empty = {
  __typename?: 'Empty';
  _?: Maybe<Scalars['Boolean']['output']>;
};

export type Everything = Cat | Dog | Image | Product | User;

export type Filter = {
  ids?: InputMaybe<Array<Scalars['ID']['input']>>;
  kind?: InputMaybe<Kind>;
  maybe?: InputMaybe<Array<InputMaybe<Scalars['String']['input']>>>;
  money?: InputMaybe<Scalars['Money']['input']>;
  names: Array<Scalars['String']['input']>;
  nested?: InputMaybe<Filter>;
  range?: Range;
};

export type Image = {
  __typename?: 'Image';
  /** @deprecated Use `url`. */
  alt?: Maybe<Scalars['String']['output']>;
  h?: Maybe<Scalars['Int']['output']>;
  url: Scalars['URL']['output'];
  w?: Maybe<Scalars['Int']['output']>;
};

export const ImageFormat = {
  Jpg: 'JPG',
  Png: 'PNG'
} as const;

export type ImageFormat = typeof ImageFormat[keyof typeof ImageFormat];
export type Item = Product | Variant;

export const Kind = {
  A1: 'A1',
  Digital: 'DIGITAL',
  Physical: 'PHYSICAL'
} as const;

export type Kind = typeof Kind[keyof typeof Kind];
export type Mutation = {
  __typename?: 'Mutation';
  createUser?: Maybe<User>;
  tag?: Maybe<Array<Node>>;
};


export type MutationCreateUserArgs = {
  input: UserInput;
};


export type MutationTagArgs = {
  ids: Array<Scalars['ID']['input']>;
  tag: Scalars['String']['input'];
};

export type N2a = Node2 & {
  __typename?: 'N2a';
  extra?: Maybe<Scalars['String']['output']>;
  id: Scalars['ID']['output'];
  parent?: Maybe<Node2>;
};

export type N2b = Node2 & {
  __typename?: 'N2b';
  id: Scalars['ID']['output'];
  other?: Maybe<Scalars['Int']['output']>;
  parent?: Maybe<Node2>;
};

/** Things with a name. */
export type Named = {
  id: Scalars['ID']['output'];
  name: Scalars['String']['output'];
};

export type Node = {
  id: Scalars['ID']['output'];
};

export type Node2 = {
  id: Scalars['ID']['output'];
  parent?: Maybe<Node2>;
};

export type PageInfo = {
  __typename?: 'PageInfo';
  end?: Maybe<Scalars['String']['output']>;
  hasNext: Scalars['Boolean']['output'];
};

export type Pet = {
  id: Scalars['ID']['output'];
};

export type Pick =
  { id: Scalars['ID']['input']; name?: never; nested?: never; }
  |  { id?: never; name: Scalars['String']['input']; nested?: never; }
  |  { id?: never; name?: never; nested: Filter; };

/** A product. */
export type Product = Named & Node & {
  __typename?: 'Product';
  collections: Array<Collection>;
  id: Scalars['ID']['output'];
  image?: Maybe<Image>;
  kind?: Maybe<Kind>;
  name: Scalars['String']['output'];
  price: Scalars['Money']['output'];
  variants: Array<Variant>;
};


/** A product. */
export type ProductVariantsArgs = {
  after?: InputMaybe<Scalars['String']['input']>;
  filter?: InputMaybe<Array<Filter>>;
  first?: InputMaybe<Scalars['Int']['input']>;
};

export type ProductConnection = {
  __typename?: 'ProductConnection';
  edges: Array<ProductEdge>;
  pageInfo: PageInfo;
};

export type ProductEdge = {
  __typename?: 'ProductEdge';
  cursor: Scalars['String']['output'];
  node: Product;
};

/** The root of the queries. */
export type Query = {
  __typename?: 'Query';
  a_b?: Maybe<Collection>;
  address?: Maybe<Address>;
  blob?: Maybe<Scalars['JSON']['output']>;
  empty?: Maybe<Empty>;
  everything: Array<Everything>;
  feed: Array<Maybe<Array<Item>>>;
  grid: Array<Array<Item>>;
  me: User;
  named?: Maybe<Array<Named>>;
  /** Any object by id. */
  node?: Maybe<Node>;
  node2?: Maybe<Node2>;
  nodes: Array<Maybe<Node>>;
  oneOf?: Maybe<Scalars['String']['output']>;
  price?: Maybe<Scalars['Money']['output']>;
  products?: Maybe<ProductConnection>;
  search: Array<SearchResult>;
  viewer?: Maybe<User>;
  when: Scalars['DateTime']['output'];
};


/** The root of the queries. */
export type QueryNodeArgs = {
  id: Scalars['ID']['input'];
};


/** The root of the queries. */
export type QueryNodesArgs = {
  ids: Array<Scalars['ID']['input']>;
};


/** The root of the queries. */
export type QueryOneOfArgs = {
  input?: InputMaybe<Pick>;
};


/** The root of the queries. */
export type QueryProductsArgs = {
  sort?: InputMaybe<SortKey>;
};


/** The root of the queries. */
export type QuerySearchArgs = {
  filter?: InputMaybe<Filter>;
  first?: InputMaybe<Scalars['Int']['input']>;
  kinds?: InputMaybe<Array<Kind>>;
  on?: Scalars['Boolean']['input'];
  q: Scalars['String']['input'];
  ratio?: InputMaybe<Scalars['Float']['input']>;
};

export type Range = {
  max: Scalars['Int']['input'];
  min: Scalars['Int']['input'];
};

export const Role = {
  Admin: 'ADMIN',
  GuestUser: 'GUEST_USER',
  User: 'USER',
  SuperUser: 'super_user'
} as const;

export type Role = typeof Role[keyof typeof Role];
export type SearchResult = Collection | Product | User;

export const SortKey = {
  Price: 'PRICE',
  Title: 'TITLE'
} as const;

export type SortKey = typeof SortKey[keyof typeof SortKey];
export type Subscription = {
  __typename?: 'Subscription';
  userAdded?: Maybe<User>;
};

export type User = Named & Node & {
  __typename?: 'User';
  address?: Maybe<Address>;
  avatar?: Maybe<Image>;
  balance?: Maybe<Scalars['Money']['output']>;
  best?: Maybe<Named>;
  createdAt: Scalars['DateTime']['output'];
  email?: Maybe<Scalars['String']['output']>;
  friends: Array<User>;
  id: Scalars['ID']['output'];
  meta?: Maybe<Scalars['JSON']['output']>;
  name: Scalars['String']['output'];
  pets?: Maybe<Array<Pet>>;
  role: Role;
  tags?: Maybe<Array<Maybe<Scalars['String']['output']>>>;
};


export type UserAvatarArgs = {
  format?: InputMaybe<ImageFormat>;
  size?: InputMaybe<Scalars['Int']['input']>;
};

export type UserInput = {
  email?: InputMaybe<Scalars['String']['input']>;
  name: Scalars['String']['input'];
  role?: InputMaybe<Role>;
  tags?: InputMaybe<Array<Scalars['String']['input']>>;
};

export type Variant = Node & {
  __typename?: 'Variant';
  id: Scalars['ID']['output'];
  product: Product;
  sku?: Maybe<Scalars['String']['output']>;
  title: Scalars['String']['output'];
};

export type VarsVariables = Exact<{
  id: Scalars['ID']['input'];
  n?: InputMaybe<Scalars['Int']['input']>;
  m?: InputMaybe<Scalars['Int']['input']>;
  l: Array<Scalars['String']['input']> | Scalars['String']['input'];
  l2?: InputMaybe<Array<InputMaybe<Scalars['String']['input']>> | InputMaybe<Scalars['String']['input']>>;
  l3?: InputMaybe<Array<Array<Scalars['Int']['input']> | Scalars['Int']['input']> | Array<Scalars['Int']['input']> | Scalars['Int']['input']>;
  f?: InputMaybe<Filter>;
  fs?: InputMaybe<Array<Filter> | Filter>;
  k?: InputMaybe<Kind>;
  ks?: InputMaybe<Array<Kind> | Kind>;
  b?: Scalars['Boolean']['input'];
  d?: InputMaybe<Scalars['DateTime']['input']>;
  j?: InputMaybe<Scalars['JSON']['input']>;
  money: Scalars['Money']['input'];
}>;


export type Vars = { __typename?: 'Query', node?: { __typename?: 'Collection', id: string } | { __typename?: 'Product', id: string } | { __typename?: 'User', id: string } | { __typename?: 'Variant', id: string } | null, search: Array<{ __typename: 'Collection' } | { __typename: 'Product' } | { __typename: 'User' }> };

export type IfaceVariables = Exact<{ [key: string]: never; }>;


export type Iface = { __typename?: 'Query', node?: { __typename?: 'Collection', id: string } | { __typename?: 'Product', name: string, price: unknown, id: string } | { __typename?: 'User', name: string, email?: string | null, id: string } | { __typename?: 'Variant', sku?: string | null, id: string } | null };

export type CondVariables = Exact<{
  a: Scalars['Boolean']['input'];
  b: Scalars['Boolean']['input'];
}>;


export type Cond = { __typename?: 'Query', me: { __typename?: 'User', id?: string, name?: string, role?: Role, friends?: Array<{ __typename?: 'User', id: string }>, addr?: { __typename?: 'Address', city?: string | null } | null }, node?: { __typename?: 'Collection', id: string } | { __typename?: 'Product', id: string } | { __typename?: 'User', name: string, id: string, friends: Array<{ __typename?: 'User', id: string }> } | { __typename?: 'Variant', id: string } | null, viewer?: { __typename?: 'User', id: string } | null, search: Array<{ __typename?: 'Collection' } | { __typename?: 'Product', price?: unknown } | { __typename?: 'User', address?: { __typename?: 'Address', city?: string | null } | null }> };

export type EnVariables = Exact<{
  k?: InputMaybe<Kind>;
  r?: InputMaybe<Role>;
}>;


export type En = { __typename?: 'Query', me: { __typename?: 'User', role: Role }, search: Array<{ __typename?: 'Collection' } | { __typename?: 'Product', kind?: Kind | null } | { __typename?: 'User' }>, address?: { __typename?: 'Address', country: CountryCode } | null };

export type ScVariables = Exact<{ [key: string]: never; }>;


export type Sc = { __typename?: 'Query', price?: unknown | null, when: string, blob?: unknown | null, me: { __typename?: 'User', balance?: unknown | null, createdAt: string, meta?: unknown | null }, viewer?: { __typename?: 'User', avatar?: { __typename?: 'Image', url: string, w?: number | null, alt?: string | null } | null } | null };

export type MergeVariables = Exact<{ [key: string]: never; }>;


export type Merge = { __typename?: 'Query', me: { __typename?: 'User', friends: Array<{ __typename?: 'User', id: string, name: string }>, address?: { __typename?: 'Address', city?: string | null, line1: string } | null }, viewer?: { __typename?: 'User', friends: Array<{ __typename?: 'User', id: string }>, f: Array<{ __typename?: 'User', name: string }> } | null };

export type UserBits = { __typename?: 'User', id: string, name: string, email?: string | null };

type NamedBits_Collection = { __typename?: 'Collection', id: string, name: string };

type NamedBits_Product = { __typename?: 'Product', id: string, name: string };

type NamedBits_User = { __typename?: 'User', id: string, name: string };

export type NamedBits = NamedBits_Collection | NamedBits_Product | NamedBits_User;

type NodeBits_Collection = { __typename?: 'Collection', id: string };

type NodeBits_Product = { __typename?: 'Product', id: string };

type NodeBits_User = { __typename?: 'User', id: string };

type NodeBits_Variant = { __typename?: 'Variant', id: string };

export type NodeBits = NodeBits_Collection | NodeBits_Product | NodeBits_User | NodeBits_Variant;

type SearchBits_Collection = { __typename?: 'Collection' };

type SearchBits_Product = { __typename?: 'Product', price: unknown };

type SearchBits_User = { __typename?: 'User', name: string };

export type SearchBits = SearchBits_Collection | SearchBits_Product | SearchBits_User;

export type ProductBits = { __typename?: 'Product', id: string, image?: { __typename?: 'Image', url: string, w?: number | null } | null, collections: Array<{ __typename?: 'Collection', id: string, name: string }> };

export type ImageBits = { __typename?: 'Image', url: string, w?: number | null };

export type FragsVariables = Exact<{ [key: string]: never; }>;


export type Frags = { __typename?: 'Query', me: { __typename?: 'User', id: string, name: string, email?: string | null, friends: Array<{ __typename?: 'User', id: string, name: string, email?: string | null }> }, node?: { __typename?: 'Collection', id: string, name: string } | { __typename?: 'Product', id: string, name: string } | { __typename?: 'User', id: string, name: string } | { __typename?: 'Variant', id: string } | null, search: Array<{ __typename?: 'Collection' } | { __typename?: 'Product', id: string, price: unknown, image?: { __typename?: 'Image', url: string, w?: number | null } | null, collections: Array<{ __typename?: 'Collection', id: string, name: string }> } | { __typename?: 'User', name: string }>, named?: Array<{ __typename?: 'Collection', id: string, name: string } | { __typename?: 'Product', id: string, name: string } | { __typename?: 'User', id: string, name: string }> | null };

export type CreateVariables = Exact<{
  input: UserInput;
}>;


export type Create = { __typename?: 'Mutation', createUser?: { __typename?: 'User', id: string, name: string } | null, tag?: Array<{ __typename?: 'Collection', id: string } | { __typename?: 'Product', id: string } | { __typename?: 'User', id: string } | { __typename?: 'Variant', id: string }> | null };

export type AddedVariables = Exact<{ [key: string]: never; }>;


export type Added = { __typename?: 'Subscription', userAdded?: { __typename?: 'User', id: string, name: string } | null };

export type AnonVariables = Exact<{ [key: string]: never; }>;


export type Anon = { __typename?: 'Query', me: { __typename?: 'User', id: string } };

export type PVariables = Exact<{ [key: string]: never; }>;


export type P = { __typename?: 'Query', products?: { __typename?: 'ProductConnection', edges: Array<{ __typename?: 'ProductEdge', cursor: string, node: { __typename?: 'Product', id: string, name: string, price: unknown, image?: { __typename?: 'Image', url: string } | null, variants: Array<{ __typename?: 'Variant', id: string, sku?: string | null }> } }>, pageInfo: { __typename?: 'PageInfo', hasNext: boolean, end?: string | null } } | null };

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
  Date: { input: string; output: string; }
  DateTime: { input: string; output: string; }
  DateTimeWithoutTimezone: { input: string; output: string; }
  Decimal: { input: string; output: string; }
  Handle: { input: string; output: string; }
  JSON: { input: unknown; output: unknown; }
  TimeWithoutTimezone: { input: string; output: string; }
  URL: { input: string; output: string; }
  Void: { input: null; output: null; }
};

export type AssociatedDiscountCode = {
  code: Scalars['String']['input'];
};

export type Attribute = {
  __typename?: 'Attribute';
  key: Scalars['String']['output'];
  value?: Maybe<Scalars['String']['output']>;
};

export type BuyerIdentity = {
  __typename?: 'BuyerIdentity';
  customer?: Maybe<Customer>;
  email?: Maybe<Scalars['String']['output']>;
  isAuthenticated: Scalars['Boolean']['output'];
  phone?: Maybe<Scalars['String']['output']>;
  purchasingCompany?: Maybe<PurchasingCompany>;
  shopUser?: Maybe<ShopUser>;
};

export type Cart = HasMetafields & {
  __typename?: 'Cart';
  attribute?: Maybe<Attribute>;
  billingAddress?: Maybe<MailingAddress>;
  buyerIdentity?: Maybe<BuyerIdentity>;
  cost: CartCost;
  deliverableLines: Array<DeliverableCartLine>;
  deliveryGroups: Array<CartDeliveryGroup>;
  discountApplications: Array<DiscountApplication>;
  lines: Array<CartLine>;
  localizedFields: Array<LocalizedField>;
  metafield?: Maybe<Metafield>;
  poNumber?: Maybe<Scalars['String']['output']>;
  retailLocation?: Maybe<Location>;
};


export type CartAttributeArgs = {
  key?: InputMaybe<Scalars['String']['input']>;
};


export type CartLocalizedFieldsArgs = {
  keys?: Array<LocalizedFieldKey>;
};


export type CartMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};

export type CartCost = {
  __typename?: 'CartCost';
  subtotalAmount: MoneyV2;
  totalAmount: MoneyV2;
  totalDutyAmount?: Maybe<MoneyV2>;
  totalTaxAmount?: Maybe<MoneyV2>;
};

export type CartDeliveryGroup = {
  __typename?: 'CartDeliveryGroup';
  cartLines: Array<CartLine>;
  deliveryAddress?: Maybe<MailingAddress>;
  deliveryOptions: Array<CartDeliveryOption>;
  discountAllocations: Array<DiscountAllocation>;
  groupType: CartDeliveryGroupType;
  id: Scalars['ID']['output'];
  selectedDeliveryOption?: Maybe<CartDeliveryOption>;
};

export enum CartDeliveryGroupType {
  OneTimePurchase = 'ONE_TIME_PURCHASE',
  Subscription = 'SUBSCRIPTION'
}

export type CartDeliveryOption = HasMetafields & {
  __typename?: 'CartDeliveryOption';
  code?: Maybe<Scalars['String']['output']>;
  cost: MoneyV2;
  deliveryMethodType: DeliveryMethod;
  description?: Maybe<Scalars['String']['output']>;
  handle: Scalars['Handle']['output'];
  metafield?: Maybe<Metafield>;
  title?: Maybe<Scalars['String']['output']>;
};


export type CartDeliveryOptionMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};

export type CartDeliveryOptionsDiscountsGenerateFetchResult = {
  request?: InputMaybe<HttpRequest>;
};

export type CartDeliveryOptionsDiscountsGenerateRunResult = {
  operations: Array<DeliveryOperation>;
};

export type CartLine = {
  __typename?: 'CartLine';
  attribute?: Maybe<Attribute>;
  cost: CartLineCost;
  discountAllocations: Array<DiscountAllocation>;
  id: Scalars['ID']['output'];
  merchandise: Merchandise;
  parentRelationship?: Maybe<CartLineParentRelationship>;
  quantity: Scalars['Int']['output'];
  sellingPlanAllocation?: Maybe<SellingPlanAllocation>;
};


export type CartLineAttributeArgs = {
  key?: InputMaybe<Scalars['String']['input']>;
};

export type CartLineCost = {
  __typename?: 'CartLineCost';
  amountPerQuantity: MoneyV2;
  compareAtAmountPerQuantity?: Maybe<MoneyV2>;
  subtotalAmount: MoneyV2;
  totalAmount: MoneyV2;
};

export type CartLineMinimumQuantity = {
  ids: Array<Scalars['ID']['input']>;
  minimumQuantity: Scalars['Int']['input'];
};

export type CartLineMinimumSubtotal = {
  ids: Array<Scalars['ID']['input']>;
  minimumAmount: Scalars['Decimal']['input'];
};

export type CartLineParentRelationship = {
  __typename?: 'CartLineParentRelationship';
  parent: CartLine;
};

export type CartLinePrerequisite = {
  id: Scalars['ID']['input'];
  quantity: Scalars['Int']['input'];
};

export type CartLineTarget = {
  id: Scalars['ID']['input'];
  quantity?: InputMaybe<Scalars['Int']['input']>;
};

export type CartLinesDiscountsGenerateFetchResult = {
  request?: InputMaybe<HttpRequest>;
};

export type CartLinesDiscountsGenerateRunResult = {
  operations: Array<CartOperation>;
};

export type CartOperation =
  { enteredDiscountCodesAccept: EnteredDiscountCodesAcceptOperation; enteredDiscountCodesReject?: never; orderDiscountsAdd?: never; productDiscountsAdd?: never; }
  |  { enteredDiscountCodesAccept?: never; enteredDiscountCodesReject: EnteredDiscountCodesRejectOperation; orderDiscountsAdd?: never; productDiscountsAdd?: never; }
  |  { enteredDiscountCodesAccept?: never; enteredDiscountCodesReject?: never; orderDiscountsAdd: OrderDiscountsAddOperation; productDiscountsAdd?: never; }
  |  { enteredDiscountCodesAccept?: never; enteredDiscountCodesReject?: never; orderDiscountsAdd?: never; productDiscountsAdd: ProductDiscountsAddOperation; };

export type CollectionMembership = {
  __typename?: 'CollectionMembership';
  collectionId: Scalars['ID']['output'];
  isMember: Scalars['Boolean']['output'];
};

export type Company = HasMetafields & {
  __typename?: 'Company';
  createdAt: Scalars['DateTime']['output'];
  externalId?: Maybe<Scalars['String']['output']>;
  id: Scalars['ID']['output'];
  metafield?: Maybe<Metafield>;
  name: Scalars['String']['output'];
  updatedAt: Scalars['DateTime']['output'];
};


export type CompanyMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};

export type CompanyContact = {
  __typename?: 'CompanyContact';
  createdAt: Scalars['DateTime']['output'];
  id: Scalars['ID']['output'];
  locale?: Maybe<Scalars['String']['output']>;
  title?: Maybe<Scalars['String']['output']>;
  updatedAt: Scalars['DateTime']['output'];
};

export type CompanyLocation = HasMetafields & {
  __typename?: 'CompanyLocation';
  createdAt: Scalars['DateTime']['output'];
  externalId?: Maybe<Scalars['String']['output']>;
  id: Scalars['ID']['output'];
  locale?: Maybe<Scalars['String']['output']>;
  metafield?: Maybe<Metafield>;
  name: Scalars['String']['output'];
  ordersCount: Scalars['Int']['output'];
  totalSpent: MoneyV2;
  updatedAt: Scalars['DateTime']['output'];
};


export type CompanyLocationMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};

export type Condition =
  { cartLineMinimumQuantity: CartLineMinimumQuantity; cartLineMinimumSubtotal?: never; orderMinimumSubtotal?: never; }
  |  { cartLineMinimumQuantity?: never; cartLineMinimumSubtotal: CartLineMinimumSubtotal; orderMinimumSubtotal?: never; }
  |  { cartLineMinimumQuantity?: never; cartLineMinimumSubtotal?: never; orderMinimumSubtotal: OrderMinimumSubtotal; };

export type Country = {
  __typename?: 'Country';
  isoCode: CountryCode;
};

export enum CountryCode {
  Ac = 'AC',
  Ad = 'AD',
  Ae = 'AE',
  Af = 'AF',
  Ag = 'AG',
  Ai = 'AI',
  Al = 'AL',
  Am = 'AM',
  An = 'AN',
  Ao = 'AO',
  Ar = 'AR',
  At = 'AT',
  Au = 'AU',
  Aw = 'AW',
  Ax = 'AX',
  Az = 'AZ',
  Ba = 'BA',
  Bb = 'BB',
  Bd = 'BD',
  Be = 'BE',
  Bf = 'BF',
  Bg = 'BG',
  Bh = 'BH',
  Bi = 'BI',
  Bj = 'BJ',
  Bl = 'BL',
  Bm = 'BM',
  Bn = 'BN',
  Bo = 'BO',
  Bq = 'BQ',
  Br = 'BR',
  Bs = 'BS',
  Bt = 'BT',
  Bv = 'BV',
  Bw = 'BW',
  By = 'BY',
  Bz = 'BZ',
  Ca = 'CA',
  Cc = 'CC',
  Cd = 'CD',
  Cf = 'CF',
  Cg = 'CG',
  Ch = 'CH',
  Ci = 'CI',
  Ck = 'CK',
  Cl = 'CL',
  Cm = 'CM',
  Cn = 'CN',
  Co = 'CO',
  Cr = 'CR',
  Cu = 'CU',
  Cv = 'CV',
  Cw = 'CW',
  Cx = 'CX',
  Cy = 'CY',
  Cz = 'CZ',
  De = 'DE',
  Dj = 'DJ',
  Dk = 'DK',
  Dm = 'DM',
  Do = 'DO',
  Dz = 'DZ',
  Ec = 'EC',
  Ee = 'EE',
  Eg = 'EG',
  Eh = 'EH',
  Er = 'ER',
  Es = 'ES',
  Et = 'ET',
  Fi = 'FI',
  Fj = 'FJ',
  Fk = 'FK',
  Fo = 'FO',
  Fr = 'FR',
  Ga = 'GA',
  Gb = 'GB',
  Gd = 'GD',
  Ge = 'GE',
  Gf = 'GF',
  Gg = 'GG',
  Gh = 'GH',
  Gi = 'GI',
  Gl = 'GL',
  Gm = 'GM',
  Gn = 'GN',
  Gp = 'GP',
  Gq = 'GQ',
  Gr = 'GR',
  Gs = 'GS',
  Gt = 'GT',
  Gw = 'GW',
  Gy = 'GY',
  Hk = 'HK',
  Hm = 'HM',
  Hn = 'HN',
  Hr = 'HR',
  Ht = 'HT',
  Hu = 'HU',
  Id = 'ID',
  Ie = 'IE',
  Il = 'IL',
  Im = 'IM',
  In = 'IN',
  Io = 'IO',
  Iq = 'IQ',
  Ir = 'IR',
  Is = 'IS',
  It = 'IT',
  Je = 'JE',
  Jm = 'JM',
  Jo = 'JO',
  Jp = 'JP',
  Ke = 'KE',
  Kg = 'KG',
  Kh = 'KH',
  Ki = 'KI',
  Km = 'KM',
  Kn = 'KN',
  Kp = 'KP',
  Kr = 'KR',
  Kw = 'KW',
  Ky = 'KY',
  Kz = 'KZ',
  La = 'LA',
  Lb = 'LB',
  Lc = 'LC',
  Li = 'LI',
  Lk = 'LK',
  Lr = 'LR',
  Ls = 'LS',
  Lt = 'LT',
  Lu = 'LU',
  Lv = 'LV',
  Ly = 'LY',
  Ma = 'MA',
  Mc = 'MC',
  Md = 'MD',
  Me = 'ME',
  Mf = 'MF',
  Mg = 'MG',
  Mk = 'MK',
  Ml = 'ML',
  Mm = 'MM',
  Mn = 'MN',
  Mo = 'MO',
  Mq = 'MQ',
  Mr = 'MR',
  Ms = 'MS',
  Mt = 'MT',
  Mu = 'MU',
  Mv = 'MV',
  Mw = 'MW',
  Mx = 'MX',
  My = 'MY',
  Mz = 'MZ',
  Na = 'NA',
  Nc = 'NC',
  Ne = 'NE',
  Nf = 'NF',
  Ng = 'NG',
  Ni = 'NI',
  Nl = 'NL',
  No = 'NO',
  Np = 'NP',
  Nr = 'NR',
  Nu = 'NU',
  Nz = 'NZ',
  Om = 'OM',
  Pa = 'PA',
  Pe = 'PE',
  Pf = 'PF',
  Pg = 'PG',
  Ph = 'PH',
  Pk = 'PK',
  Pl = 'PL',
  Pm = 'PM',
  Pn = 'PN',
  Ps = 'PS',
  Pt = 'PT',
  Py = 'PY',
  Qa = 'QA',
  Re = 'RE',
  Ro = 'RO',
  Rs = 'RS',
  Ru = 'RU',
  Rw = 'RW',
  Sa = 'SA',
  Sb = 'SB',
  Sc = 'SC',
  Sd = 'SD',
  Se = 'SE',
  Sg = 'SG',
  Sh = 'SH',
  Si = 'SI',
  Sj = 'SJ',
  Sk = 'SK',
  Sl = 'SL',
  Sm = 'SM',
  Sn = 'SN',
  So = 'SO',
  Sr = 'SR',
  Ss = 'SS',
  St = 'ST',
  Sv = 'SV',
  Sx = 'SX',
  Sy = 'SY',
  Sz = 'SZ',
  Ta = 'TA',
  Tc = 'TC',
  Td = 'TD',
  Tf = 'TF',
  Tg = 'TG',
  Th = 'TH',
  Tj = 'TJ',
  Tk = 'TK',
  Tl = 'TL',
  Tm = 'TM',
  Tn = 'TN',
  To = 'TO',
  Tr = 'TR',
  Tt = 'TT',
  Tv = 'TV',
  Tw = 'TW',
  Tz = 'TZ',
  Ua = 'UA',
  Ug = 'UG',
  Um = 'UM',
  Us = 'US',
  Uy = 'UY',
  Uz = 'UZ',
  Va = 'VA',
  Vc = 'VC',
  Ve = 'VE',
  Vg = 'VG',
  Vn = 'VN',
  Vu = 'VU',
  Wf = 'WF',
  Ws = 'WS',
  Xk = 'XK',
  Ye = 'YE',
  Yt = 'YT',
  Za = 'ZA',
  Zm = 'ZM',
  Zw = 'ZW',
  Zz = 'ZZ'
}

export enum CurrencyCode {
  Aed = 'AED',
  Afn = 'AFN',
  All = 'ALL',
  Amd = 'AMD',
  /** @deprecated Use `XCG` instead. */
  Ang = 'ANG',
  Aoa = 'AOA',
  Ars = 'ARS',
  Aud = 'AUD',
  Awg = 'AWG',
  Azn = 'AZN',
  Bam = 'BAM',
  Bbd = 'BBD',
  Bdt = 'BDT',
  Bgn = 'BGN',
  Bhd = 'BHD',
  Bif = 'BIF',
  Bmd = 'BMD',
  Bnd = 'BND',
  Bob = 'BOB',
  Brl = 'BRL',
  Bsd = 'BSD',
  Btn = 'BTN',
  Bwp = 'BWP',
  Byn = 'BYN',
  /** @deprecated Use `BYN` instead. */
  Byr = 'BYR',
  Bzd = 'BZD',
  Cad = 'CAD',
  Cdf = 'CDF',
  Chf = 'CHF',
  Clp = 'CLP',
  Cny = 'CNY',
  Cop = 'COP',
  Crc = 'CRC',
  Cve = 'CVE',
  Czk = 'CZK',
  Djf = 'DJF',
  Dkk = 'DKK',
  Dop = 'DOP',
  Dzd = 'DZD',
  Egp = 'EGP',
  Ern = 'ERN',
  Etb = 'ETB',
  Eur = 'EUR',
  Fjd = 'FJD',
  Fkp = 'FKP',
  Gbp = 'GBP',
  Gel = 'GEL',
  Ghs = 'GHS',
  Gip = 'GIP',
  Gmd = 'GMD',
  Gnf = 'GNF',
  Gtq = 'GTQ',
  Gyd = 'GYD',
  Hkd = 'HKD',
  Hnl = 'HNL',
  Hrk = 'HRK',
  Htg = 'HTG',
  Huf = 'HUF',
  Idr = 'IDR',
  Ils = 'ILS',
  Inr = 'INR',
  Iqd = 'IQD',
  Irr = 'IRR',
  Isk = 'ISK',
  Jep = 'JEP',
  Jmd = 'JMD',
  Jod = 'JOD',
  Jpy = 'JPY',
  Kes = 'KES',
  Kgs = 'KGS',
  Khr = 'KHR',
  Kid = 'KID',
  Kmf = 'KMF',
  Krw = 'KRW',
  Kwd = 'KWD',
  Kyd = 'KYD',
  Kzt = 'KZT',
  Lak = 'LAK',
  Lbp = 'LBP',
  Lkr = 'LKR',
  Lrd = 'LRD',
  Lsl = 'LSL',
  Ltl = 'LTL',
  Lvl = 'LVL',
  Lyd = 'LYD',
  Mad = 'MAD',
  Mdl = 'MDL',
  Mga = 'MGA',
  Mkd = 'MKD',
  Mmk = 'MMK',
  Mnt = 'MNT',
  Mop = 'MOP',
  Mru = 'MRU',
  Mur = 'MUR',
  Mvr = 'MVR',
  Mwk = 'MWK',
  Mxn = 'MXN',
  Myr = 'MYR',
  Mzn = 'MZN',
  Nad = 'NAD',
  Ngn = 'NGN',
  Nio = 'NIO',
  Nok = 'NOK',
  Npr = 'NPR',
  Nzd = 'NZD',
  Omr = 'OMR',
  Pab = 'PAB',
  Pen = 'PEN',
  Pgk = 'PGK',
  Php = 'PHP',
  Pkr = 'PKR',
  Pln = 'PLN',
  Pyg = 'PYG',
  Qar = 'QAR',
  Ron = 'RON',
  Rsd = 'RSD',
  Rub = 'RUB',
  Rwf = 'RWF',
  Sar = 'SAR',
  Sbd = 'SBD',
  Scr = 'SCR',
  Sdg = 'SDG',
  Sek = 'SEK',
  Sgd = 'SGD',
  Shp = 'SHP',
  Sll = 'SLL',
  Sos = 'SOS',
  Srd = 'SRD',
  Ssp = 'SSP',
  /** @deprecated Use `STN` instead. */
  Std = 'STD',
  Stn = 'STN',
  Syp = 'SYP',
  Szl = 'SZL',
  Thb = 'THB',
  Tjs = 'TJS',
  Tmt = 'TMT',
  Tnd = 'TND',
  Top = 'TOP',
  Try = 'TRY',
  Ttd = 'TTD',
  Twd = 'TWD',
  Tzs = 'TZS',
  Uah = 'UAH',
  Ugx = 'UGX',
  Usd = 'USD',
  Usdc = 'USDC',
  Uyu = 'UYU',
  Uzs = 'UZS',
  Ved = 'VED',
  /** @deprecated Use `VES` instead. */
  Vef = 'VEF',
  Ves = 'VES',
  Vnd = 'VND',
  Vuv = 'VUV',
  Wst = 'WST',
  Xaf = 'XAF',
  Xcd = 'XCD',
  Xcg = 'XCG',
  Xof = 'XOF',
  Xpf = 'XPF',
  Xxx = 'XXX',
  Yer = 'YER',
  Zar = 'ZAR',
  Zmw = 'ZMW'
}

export type CustomProduct = {
  __typename?: 'CustomProduct';
  isGiftCard: Scalars['Boolean']['output'];
  requiresShipping: Scalars['Boolean']['output'];
  title: Scalars['String']['output'];
  weight?: Maybe<Scalars['Float']['output']>;
  weightUnit: WeightUnit;
};

export type Customer = HasMetafields & {
  __typename?: 'Customer';
  amountSpent: MoneyV2;
  createdAt: Scalars['DateTime']['output'];
  displayName: Scalars['String']['output'];
  email?: Maybe<Scalars['String']['output']>;
  firstName?: Maybe<Scalars['String']['output']>;
  hasAnyTag: Scalars['Boolean']['output'];
  hasTags: Array<HasTagResponse>;
  id: Scalars['ID']['output'];
  lastName?: Maybe<Scalars['String']['output']>;
  metafield?: Maybe<Metafield>;
  numberOfOrders: Scalars['Int']['output'];
};


export type CustomerHasAnyTagArgs = {
  tags?: Array<Scalars['String']['input']>;
};


export type CustomerHasTagsArgs = {
  tags?: Array<Scalars['String']['input']>;
};


export type CustomerMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};

export type DeliverableCartLine = {
  __typename?: 'DeliverableCartLine';
  attribute?: Maybe<Attribute>;
  id: Scalars['ID']['output'];
  merchandise: Merchandise;
  quantity: Scalars['Int']['output'];
};


export type DeliverableCartLineAttributeArgs = {
  key?: InputMaybe<Scalars['String']['input']>;
};

export type DeliveryDiscountCandidate = {
  associatedDiscountCode?: InputMaybe<AssociatedDiscountCode>;
  message?: InputMaybe<Scalars['String']['input']>;
  targets: Array<DeliveryDiscountCandidateTarget>;
  value: DeliveryDiscountCandidateValue;
};

export type DeliveryDiscountCandidateTarget =
  { deliveryGroup: DeliveryGroupTarget; deliveryOption?: never; }
  |  { deliveryGroup?: never; deliveryOption: DeliveryOptionTarget; };

export type DeliveryDiscountCandidateValue =
  { fixedAmount: FixedAmount; percentage?: never; }
  |  { fixedAmount?: never; percentage: Percentage; };

export enum DeliveryDiscountSelectionStrategy {
  All = 'ALL'
}

export type DeliveryDiscountsAddOperation = {
  candidates: Array<DeliveryDiscountCandidate>;
  selectionStrategy: DeliveryDiscountSelectionStrategy;
};

export type DeliveryGroupTarget = {
  id: Scalars['ID']['input'];
};

export enum DeliveryMethod {
  Local = 'LOCAL',
  None = 'NONE',
  PickupPoint = 'PICKUP_POINT',
  PickUp = 'PICK_UP',
  Retail = 'RETAIL',
  Shipping = 'SHIPPING'
}

export type DeliveryOperation =
  { deliveryDiscountsAdd: DeliveryDiscountsAddOperation; enteredDiscountCodesAccept?: never; enteredDiscountCodesReject?: never; }
  |  { deliveryDiscountsAdd?: never; enteredDiscountCodesAccept: EnteredDiscountCodesAcceptOperation; enteredDiscountCodesReject?: never; }
  |  { deliveryDiscountsAdd?: never; enteredDiscountCodesAccept?: never; enteredDiscountCodesReject: EnteredDiscountCodesRejectOperation; };

export type DeliveryOptionTarget = {
  handle: Scalars['Handle']['input'];
};

export type Discount = HasMetafields & {
  __typename?: 'Discount';
  discountClasses: Array<DiscountClass>;
  metafield?: Maybe<Metafield>;
};


export type DiscountMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};

export type DiscountAllocation = {
  __typename?: 'DiscountAllocation';
  discountApplication: DiscountApplication;
  discountedAmount: MoneyV2;
};

export type DiscountApplication = HasMetafields & {
  __typename?: 'DiscountApplication';
  allocationMethod: DiscountApplicationAllocationMethod;
  metafield?: Maybe<Metafield>;
  targetSelection: DiscountApplicationTargetSelection;
  targetType: DiscountApplicationTarget;
  totalAllocatedAmount: MoneyV2;
  value: PricingValue;
};


export type DiscountApplicationMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};

export enum DiscountApplicationAllocationMethod {
  Across = 'ACROSS',
  Each = 'EACH'
}

export enum DiscountApplicationTarget {
  LineItem = 'LINE_ITEM',
  ShippingLine = 'SHIPPING_LINE'
}

export enum DiscountApplicationTargetSelection {
  All = 'ALL',
  Entitled = 'ENTITLED',
  Explicit = 'EXPLICIT'
}

export enum DiscountClass {
  Order = 'ORDER',
  Product = 'PRODUCT',
  Shipping = 'SHIPPING'
}

export type DiscountCode = {
  code: Scalars['String']['input'];
};

export type EnteredDiscountCode = {
  __typename?: 'EnteredDiscountCode';
  code: Scalars['String']['output'];
  rejectable: Scalars['Boolean']['output'];
};

export type EnteredDiscountCodesAcceptOperation = {
  codes: Array<DiscountCode>;
};

export type EnteredDiscountCodesRejectOperation = {
  codes: Array<RejectedDiscountCode>;
  message: Scalars['String']['input'];
};

export type FixedAmount = {
  amount: Scalars['Decimal']['input'];
};

export type HasMetafields = {
  metafield?: Maybe<Metafield>;
};


export type HasMetafieldsMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};

export type HasTagResponse = {
  __typename?: 'HasTagResponse';
  hasTag: Scalars['Boolean']['output'];
  tag: Scalars['String']['output'];
};

export type HttpRequest = {
  body?: InputMaybe<Scalars['String']['input']>;
  headers: Array<HttpRequestHeader>;
  jsonBody?: InputMaybe<Scalars['JSON']['input']>;
  method: HttpRequestMethod;
  policy: HttpRequestPolicy;
  url: Scalars['URL']['input'];
};

export type HttpRequestHeader = {
  name: Scalars['String']['input'];
  value: Scalars['String']['input'];
};

export enum HttpRequestMethod {
  Get = 'GET',
  Post = 'POST'
}

export type HttpRequestPolicy = {
  readTimeoutMs: Scalars['Int']['input'];
};

export type HttpResponse = {
  __typename?: 'HttpResponse';
  body?: Maybe<Scalars['String']['output']>;
  header?: Maybe<HttpResponseHeader>;
  /** @deprecated Use `header` instead. */
  headers: Array<HttpResponseHeader>;
  jsonBody?: Maybe<Scalars['JSON']['output']>;
  status: Scalars['Int']['output'];
};


export type HttpResponseHeaderArgs = {
  name: Scalars['String']['input'];
};

export type HttpResponseHeader = {
  __typename?: 'HttpResponseHeader';
  name: Scalars['String']['output'];
  value: Scalars['String']['output'];
};

export type Input = {
  __typename?: 'Input';
  cart: Cart;
  discount: Discount;
  enteredDiscountCodes: Array<EnteredDiscountCode>;
  fetchResult?: Maybe<HttpResponse>;
  localization: Localization;
  presentmentCurrencyRate: Scalars['Decimal']['output'];
  shop: Shop;
  triggeringDiscountCode?: Maybe<Scalars['String']['output']>;
};

export type Language = {
  __typename?: 'Language';
  isoCode: LanguageCode;
};

export enum LanguageCode {
  Af = 'AF',
  Ak = 'AK',
  Am = 'AM',
  Ar = 'AR',
  As = 'AS',
  Az = 'AZ',
  Be = 'BE',
  Bg = 'BG',
  Bm = 'BM',
  Bn = 'BN',
  Bo = 'BO',
  Br = 'BR',
  Bs = 'BS',
  Ca = 'CA',
  Ce = 'CE',
  Ckb = 'CKB',
  Cs = 'CS',
  Cu = 'CU',
  Cy = 'CY',
  Da = 'DA',
  De = 'DE',
  Dz = 'DZ',
  Ee = 'EE',
  El = 'EL',
  En = 'EN',
  Eo = 'EO',
  Es = 'ES',
  Et = 'ET',
  Eu = 'EU',
  Fa = 'FA',
  Ff = 'FF',
  Fi = 'FI',
  Fil = 'FIL',
  Fo = 'FO',
  Fr = 'FR',
  Fy = 'FY',
  Ga = 'GA',
  Gd = 'GD',
  Gl = 'GL',
  Gu = 'GU',
  Gv = 'GV',
  Ha = 'HA',
  He = 'HE',
  Hi = 'HI',
  Hr = 'HR',
  Hu = 'HU',
  Hy = 'HY',
  Ia = 'IA',
  Id = 'ID',
  Ig = 'IG',
  Ii = 'II',
  Is = 'IS',
  It = 'IT',
  Ja = 'JA',
  Jv = 'JV',
  Ka = 'KA',
  Ki = 'KI',
  Kk = 'KK',
  Kl = 'KL',
  Km = 'KM',
  Kn = 'KN',
  Ko = 'KO',
  Ks = 'KS',
  Ku = 'KU',
  Kw = 'KW',
  Ky = 'KY',
  Lb = 'LB',
  Lg = 'LG',
  Ln = 'LN',
  Lo = 'LO',
  Lt = 'LT',
  Lu = 'LU',
  Lv = 'LV',
  Mg = 'MG',
  Mi = 'MI',
  Mk = 'MK',
  Ml = 'ML',
  Mn = 'MN',
  Mr = 'MR',
  Ms = 'MS',
  Mt = 'MT',
  My = 'MY',
  Nb = 'NB',
  Nd = 'ND',
  Ne = 'NE',
  Nl = 'NL',
  Nn = 'NN',
  No = 'NO',
  Om = 'OM',
  Or = 'OR',
  Os = 'OS',
  Pa = 'PA',
  Pl = 'PL',
  Ps = 'PS',
  Pt = 'PT',
  PtBr = 'PT_BR',
  PtPt = 'PT_PT',
  Qu = 'QU',
  Rm = 'RM',
  Rn = 'RN',
  Ro = 'RO',
  Ru = 'RU',
  Rw = 'RW',
  Sa = 'SA',
  Sc = 'SC',
  Sd = 'SD',
  Se = 'SE',
  Sg = 'SG',
  Si = 'SI',
  Sk = 'SK',
  Sl = 'SL',
  Sn = 'SN',
  So = 'SO',
  Sq = 'SQ',
  Sr = 'SR',
  Su = 'SU',
  Sv = 'SV',
  Sw = 'SW',
  Ta = 'TA',
  Te = 'TE',
  Tg = 'TG',
  Th = 'TH',
  Ti = 'TI',
  Tk = 'TK',
  To = 'TO',
  Tr = 'TR',
  Tt = 'TT',
  Ug = 'UG',
  Uk = 'UK',
  Ur = 'UR',
  Uz = 'UZ',
  Vi = 'VI',
  Vo = 'VO',
  Wo = 'WO',
  Xh = 'XH',
  Yi = 'YI',
  Yo = 'YO',
  Zh = 'ZH',
  ZhCn = 'ZH_CN',
  ZhTw = 'ZH_TW',
  Zu = 'ZU'
}

export type LocalTime = {
  __typename?: 'LocalTime';
  date: Scalars['Date']['output'];
  dateTimeAfter: Scalars['Boolean']['output'];
  dateTimeBefore: Scalars['Boolean']['output'];
  dateTimeBetween: Scalars['Boolean']['output'];
  timeAfter: Scalars['Boolean']['output'];
  timeBefore: Scalars['Boolean']['output'];
  timeBetween: Scalars['Boolean']['output'];
};


export type LocalTimeDateTimeAfterArgs = {
  dateTime: Scalars['DateTimeWithoutTimezone']['input'];
};


export type LocalTimeDateTimeBeforeArgs = {
  dateTime: Scalars['DateTimeWithoutTimezone']['input'];
};


export type LocalTimeDateTimeBetweenArgs = {
  endDateTime: Scalars['DateTimeWithoutTimezone']['input'];
  startDateTime: Scalars['DateTimeWithoutTimezone']['input'];
};


export type LocalTimeTimeAfterArgs = {
  time: Scalars['TimeWithoutTimezone']['input'];
};


export type LocalTimeTimeBeforeArgs = {
  time: Scalars['TimeWithoutTimezone']['input'];
};


export type LocalTimeTimeBetweenArgs = {
  endTime: Scalars['TimeWithoutTimezone']['input'];
  startTime: Scalars['TimeWithoutTimezone']['input'];
};

export type Localization = {
  __typename?: 'Localization';
  country: Country;
  language: Language;
  /** @deprecated This `market` field will be removed in a future version of the API. */
  market: Market;
};

export type LocalizedField = {
  __typename?: 'LocalizedField';
  key: LocalizedFieldKey;
  title: Scalars['String']['output'];
  value?: Maybe<Scalars['String']['output']>;
};

export enum LocalizedFieldKey {
  ShippingCredentialBr = 'SHIPPING_CREDENTIAL_BR',
  ShippingCredentialCl = 'SHIPPING_CREDENTIAL_CL',
  ShippingCredentialCn = 'SHIPPING_CREDENTIAL_CN',
  ShippingCredentialCo = 'SHIPPING_CREDENTIAL_CO',
  ShippingCredentialCr = 'SHIPPING_CREDENTIAL_CR',
  ShippingCredentialEc = 'SHIPPING_CREDENTIAL_EC',
  ShippingCredentialEs = 'SHIPPING_CREDENTIAL_ES',
  ShippingCredentialGt = 'SHIPPING_CREDENTIAL_GT',
  ShippingCredentialId = 'SHIPPING_CREDENTIAL_ID',
  ShippingCredentialKr = 'SHIPPING_CREDENTIAL_KR',
  ShippingCredentialMx = 'SHIPPING_CREDENTIAL_MX',
  ShippingCredentialMy = 'SHIPPING_CREDENTIAL_MY',
  ShippingCredentialPe = 'SHIPPING_CREDENTIAL_PE',
  ShippingCredentialPt = 'SHIPPING_CREDENTIAL_PT',
  ShippingCredentialPy = 'SHIPPING_CREDENTIAL_PY',
  ShippingCredentialTr = 'SHIPPING_CREDENTIAL_TR',
  ShippingCredentialTw = 'SHIPPING_CREDENTIAL_TW',
  ShippingCredentialTypeCo = 'SHIPPING_CREDENTIAL_TYPE_CO',
  TaxCredentialBr = 'TAX_CREDENTIAL_BR',
  TaxCredentialCl = 'TAX_CREDENTIAL_CL',
  TaxCredentialCo = 'TAX_CREDENTIAL_CO',
  TaxCredentialCr = 'TAX_CREDENTIAL_CR',
  TaxCredentialEc = 'TAX_CREDENTIAL_EC',
  TaxCredentialEs = 'TAX_CREDENTIAL_ES',
  TaxCredentialGt = 'TAX_CREDENTIAL_GT',
  TaxCredentialId = 'TAX_CREDENTIAL_ID',
  TaxCredentialIt = 'TAX_CREDENTIAL_IT',
  TaxCredentialMx = 'TAX_CREDENTIAL_MX',
  TaxCredentialMy = 'TAX_CREDENTIAL_MY',
  TaxCredentialPe = 'TAX_CREDENTIAL_PE',
  TaxCredentialPt = 'TAX_CREDENTIAL_PT',
  TaxCredentialPy = 'TAX_CREDENTIAL_PY',
  TaxCredentialTr = 'TAX_CREDENTIAL_TR',
  TaxCredentialTypeCo = 'TAX_CREDENTIAL_TYPE_CO',
  TaxCredentialTypeMx = 'TAX_CREDENTIAL_TYPE_MX',
  TaxCredentialUseMx = 'TAX_CREDENTIAL_USE_MX',
  TaxEmailIt = 'TAX_EMAIL_IT'
}

export type Location = HasMetafields & {
  __typename?: 'Location';
  address: LocationAddress;
  handle: Scalars['Handle']['output'];
  id: Scalars['ID']['output'];
  metafield?: Maybe<Metafield>;
  name: Scalars['String']['output'];
};


export type LocationMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};

export type LocationAddress = {
  __typename?: 'LocationAddress';
  address1?: Maybe<Scalars['String']['output']>;
  address2?: Maybe<Scalars['String']['output']>;
  city?: Maybe<Scalars['String']['output']>;
  country?: Maybe<Scalars['String']['output']>;
  countryCode?: Maybe<Scalars['String']['output']>;
  formatted: Array<Scalars['String']['output']>;
  latitude?: Maybe<Scalars['Float']['output']>;
  longitude?: Maybe<Scalars['Float']['output']>;
  phone?: Maybe<Scalars['String']['output']>;
  province?: Maybe<Scalars['String']['output']>;
  provinceCode?: Maybe<Scalars['String']['output']>;
  zip?: Maybe<Scalars['String']['output']>;
};

export type MailingAddress = {
  __typename?: 'MailingAddress';
  address1?: Maybe<Scalars['String']['output']>;
  address2?: Maybe<Scalars['String']['output']>;
  city?: Maybe<Scalars['String']['output']>;
  company?: Maybe<Scalars['String']['output']>;
  countryCode?: Maybe<CountryCode>;
  firstName?: Maybe<Scalars['String']['output']>;
  lastName?: Maybe<Scalars['String']['output']>;
  latitude?: Maybe<Scalars['Float']['output']>;
  longitude?: Maybe<Scalars['Float']['output']>;
  /** @deprecated This `market` field will be removed in a future version of the API. */
  market?: Maybe<Market>;
  name?: Maybe<Scalars['String']['output']>;
  phone?: Maybe<Scalars['String']['output']>;
  provinceCode?: Maybe<Scalars['String']['output']>;
  zip?: Maybe<Scalars['String']['output']>;
};

export type Market = HasMetafields & {
  __typename?: 'Market';
  handle: Scalars['Handle']['output'];
  id: Scalars['ID']['output'];
  metafield?: Maybe<Metafield>;
  regions: Array<MarketRegion>;
};


export type MarketMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};

export type MarketRegion = {
  name?: Maybe<Scalars['String']['output']>;
};

export type MarketRegionCountry = MarketRegion & {
  __typename?: 'MarketRegionCountry';
  code: CountryCode;
  name: Scalars['String']['output'];
};

export type Merchandise = CustomProduct | ProductVariant;

export type Metafield = {
  __typename?: 'Metafield';
  jsonValue: Scalars['JSON']['output'];
  type: Scalars['String']['output'];
  value: Scalars['String']['output'];
};

export type Metaobject = {
  __typename?: 'Metaobject';
  field?: Maybe<MetaobjectField>;
  handle: Scalars['String']['output'];
  type: Scalars['String']['output'];
};


export type MetaobjectFieldArgs = {
  key: Scalars['String']['input'];
};

export type MetaobjectField = {
  __typename?: 'MetaobjectField';
  jsonValue?: Maybe<Scalars['JSON']['output']>;
  key: Scalars['String']['output'];
  type: Scalars['String']['output'];
  value?: Maybe<Scalars['String']['output']>;
};

export type MetaobjectHandleInput = {
  handle: Scalars['String']['input'];
  type: Scalars['String']['input'];
};

export type MoneyV2 = {
  __typename?: 'MoneyV2';
  amount: Scalars['Decimal']['output'];
  currencyCode: CurrencyCode;
};

export type MutationRoot = {
  __typename?: 'MutationRoot';
  cartDeliveryOptionsDiscountsGenerateFetch: Scalars['Void']['output'];
  cartDeliveryOptionsDiscountsGenerateRun: Scalars['Void']['output'];
  cartLinesDiscountsGenerateFetch: Scalars['Void']['output'];
  cartLinesDiscountsGenerateRun: Scalars['Void']['output'];
};


export type MutationRootCartDeliveryOptionsDiscountsGenerateFetchArgs = {
  result: CartDeliveryOptionsDiscountsGenerateFetchResult;
};


export type MutationRootCartDeliveryOptionsDiscountsGenerateRunArgs = {
  result: CartDeliveryOptionsDiscountsGenerateRunResult;
};


export type MutationRootCartLinesDiscountsGenerateFetchArgs = {
  result: CartLinesDiscountsGenerateFetchResult;
};


export type MutationRootCartLinesDiscountsGenerateRunArgs = {
  result: CartLinesDiscountsGenerateRunResult;
};

export type OrderDiscountCandidate = {
  associatedDiscountCode?: InputMaybe<AssociatedDiscountCode>;
  conditions?: InputMaybe<Array<Condition>>;
  message?: InputMaybe<Scalars['String']['input']>;
  targets: Array<OrderDiscountCandidateTarget>;
  value: OrderDiscountCandidateValue;
};

export type OrderDiscountCandidateTarget =
  { orderSubtotal: OrderSubtotalTarget; };

export type OrderDiscountCandidateValue =
  { fixedAmount: FixedAmount; percentage?: never; }
  |  { fixedAmount?: never; percentage: Percentage; };

export enum OrderDiscountSelectionStrategy {
  First = 'FIRST',
  Maximum = 'MAXIMUM'
}

export type OrderDiscountsAddOperation = {
  candidates: Array<OrderDiscountCandidate>;
  selectionStrategy: OrderDiscountSelectionStrategy;
};

export type OrderMinimumSubtotal = {
  excludedCartLineIds: Array<Scalars['ID']['input']>;
  minimumAmount: Scalars['Decimal']['input'];
};

export type OrderSubtotalTarget = {
  excludedCartLineIds: Array<Scalars['ID']['input']>;
};

export type Percentage = {
  value: Scalars['Decimal']['input'];
};

export type PricingPercentageValue = {
  __typename?: 'PricingPercentageValue';
  value: Scalars['Decimal']['output'];
};

export type PricingValue = MoneyV2 | PricingPercentageValue;

export type Product = HasMetafields & {
  __typename?: 'Product';
  handle: Scalars['Handle']['output'];
  hasAnyTag: Scalars['Boolean']['output'];
  hasTags: Array<HasTagResponse>;
  id: Scalars['ID']['output'];
  inAnyCollection: Scalars['Boolean']['output'];
  inCollections: Array<CollectionMembership>;
  isGiftCard: Scalars['Boolean']['output'];
  metafield?: Maybe<Metafield>;
  productType?: Maybe<Scalars['String']['output']>;
  title: Scalars['String']['output'];
  vendor?: Maybe<Scalars['String']['output']>;
};


export type ProductHasAnyTagArgs = {
  tags?: Array<Scalars['String']['input']>;
};


export type ProductHasTagsArgs = {
  tags?: Array<Scalars['String']['input']>;
};


export type ProductInAnyCollectionArgs = {
  ids?: Array<Scalars['ID']['input']>;
};


export type ProductInCollectionsArgs = {
  ids?: Array<Scalars['ID']['input']>;
};


export type ProductMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};

export type ProductDiscountCandidate = {
  associatedDiscountCode?: InputMaybe<AssociatedDiscountCode>;
  message?: InputMaybe<Scalars['String']['input']>;
  prerequisites?: InputMaybe<Array<ProductDiscountCandidatePrerequisite>>;
  targets: Array<ProductDiscountCandidateTarget>;
  value: ProductDiscountCandidateValue;
};

export type ProductDiscountCandidateFixedAmount = {
  amount: Scalars['Decimal']['input'];
  appliesToEachItem?: InputMaybe<Scalars['Boolean']['input']>;
};

export type ProductDiscountCandidatePrerequisite = {
  cartLine?: InputMaybe<CartLinePrerequisite>;
};

export type ProductDiscountCandidateTarget =
  { cartLine: CartLineTarget; };

export type ProductDiscountCandidateValue =
  { fixedAmount: ProductDiscountCandidateFixedAmount; percentage?: never; }
  |  { fixedAmount?: never; percentage: Percentage; };

export enum ProductDiscountSelectionStrategy {
  All = 'ALL',
  First = 'FIRST',
  Maximum = 'MAXIMUM'
}

export type ProductDiscountsAddOperation = {
  candidates: Array<ProductDiscountCandidate>;
  selectionStrategy: ProductDiscountSelectionStrategy;
};

export type ProductVariant = HasMetafields & {
  __typename?: 'ProductVariant';
  id: Scalars['ID']['output'];
  inAnyCollection: Scalars['Boolean']['output'];
  inCollections: Array<CollectionMembership>;
  metafield?: Maybe<Metafield>;
  product: Product;
  requiresShipping: Scalars['Boolean']['output'];
  sku?: Maybe<Scalars['String']['output']>;
  title?: Maybe<Scalars['String']['output']>;
  weight?: Maybe<Scalars['Float']['output']>;
  weightUnit: WeightUnit;
};


export type ProductVariantInAnyCollectionArgs = {
  ids?: Array<Scalars['ID']['input']>;
};


export type ProductVariantInCollectionsArgs = {
  ids?: Array<Scalars['ID']['input']>;
};


export type ProductVariantMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};

export type PurchasingCompany = {
  __typename?: 'PurchasingCompany';
  company: Company;
  contact?: Maybe<CompanyContact>;
  location: CompanyLocation;
};

export type RejectedDiscountCode = {
  code: Scalars['String']['input'];
};

export type SellingPlan = HasMetafields & {
  __typename?: 'SellingPlan';
  description?: Maybe<Scalars['String']['output']>;
  id: Scalars['ID']['output'];
  metafield?: Maybe<Metafield>;
  name: Scalars['String']['output'];
  recurringDeliveries: Scalars['Boolean']['output'];
};


export type SellingPlanMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};

export type SellingPlanAllocation = {
  __typename?: 'SellingPlanAllocation';
  priceAdjustments: Array<SellingPlanAllocationPriceAdjustment>;
  sellingPlan: SellingPlan;
};

export type SellingPlanAllocationPriceAdjustment = {
  __typename?: 'SellingPlanAllocationPriceAdjustment';
  perDeliveryPrice: MoneyV2;
  price: MoneyV2;
};

export type Shop = HasMetafields & {
  __typename?: 'Shop';
  localTime: LocalTime;
  metafield?: Maybe<Metafield>;
  metaobject?: Maybe<Metaobject>;
};


export type ShopMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};


export type ShopMetaobjectArgs = {
  handle?: InputMaybe<MetaobjectHandleInput>;
  id?: InputMaybe<Scalars['ID']['input']>;
};

export type ShopUser = HasMetafields & {
  __typename?: 'ShopUser';
  metafield?: Maybe<Metafield>;
};


export type ShopUserMetafieldArgs = {
  key: Scalars['String']['input'];
  namespace?: InputMaybe<Scalars['String']['input']>;
};

export enum WeightUnit {
  Grams = 'GRAMS',
  Kilograms = 'KILOGRAMS',
  Ounces = 'OUNCES',
  Pounds = 'POUNDS'
}

export type CartInputVariables = Exact<{ [key: string]: never; }>;


export type CartInput = { __typename?: 'Input', cart: { __typename?: 'Cart', lines: Array<{ __typename?: 'CartLine', id: string, quantity: number, merchandise: { __typename: 'CustomProduct' } | { __typename: 'ProductVariant', product: { __typename?: 'Product', title: string } } }> }, discount: { __typename?: 'Discount', discountClasses: Array<DiscountClass>, metafield?: { __typename?: 'Metafield', jsonValue: unknown } | null } };
